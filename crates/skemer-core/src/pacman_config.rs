//! Read and rewrite the small, allowlisted parts of `pacman.conf` that the
//! settings UI exposes.
//!
//! Skemer never edits include files it does not own. Repository provenance is
//! tracked so the helper can refuse to modify a package-owned drop-in such as
//! Veldmuis' `/etc/pacman.conf.d/veldmuis.conf`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// One repository section found in the effective configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub name: String,
    pub enabled: bool,
    pub source_file: PathBuf,
    pub line: usize,
}

/// Effective view of `pacman.conf` with enough provenance to decide what may be
/// edited safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigView {
    pub main_file: PathBuf,
    pub repositories: Vec<RepoEntry>,
    pub includes: Vec<PathBuf>,
    pub has_commented_multilib: bool,
    pub commented_multilib_source: Option<PathBuf>,
    pub parallel_downloads: Option<u32>,
    pub parallel_downloads_source: Option<PathBuf>,
    pub pacnew: bool,
    pub warnings: Vec<String>,
}

impl ConfigView {
    pub fn multilib(&self) -> Option<&RepoEntry> {
        self.repositories
            .iter()
            .find(|repo| repo.name == "multilib")
    }
}

/// Inspect the pacman configuration under `root`.
pub fn inspect(root: &Path) -> Result<ConfigView> {
    let main_file = root.join("etc/pacman.conf");
    let mut view = ConfigView {
        main_file: main_file.clone(),
        repositories: Vec::new(),
        includes: Vec::new(),
        has_commented_multilib: false,
        commented_multilib_source: None,
        parallel_downloads: None,
        parallel_downloads_source: None,
        pacnew: main_file.with_file_name("pacman.conf.pacnew").exists(),
        warnings: Vec::new(),
    };
    inspect_file(&main_file, &mut view, 0)?;
    Ok(view)
}

fn inspect_file(path: &Path, view: &mut ConfigView, depth: usize) -> Result<()> {
    if depth > 16 {
        anyhow::bail!(
            "pacman.conf Include nesting is too deep at {}",
            path.display()
        );
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if depth == 0 {
                return Err(error)
                    .with_context(|| format!("read pacman configuration {}", path.display()));
            }
            view.warnings.push(format!(
                "Could not read included configuration {}: {error}",
                path.display()
            ));
            return Ok(());
        }
    };

    let mut section: Option<String> = None;
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((name, commented)) = section_header(line) {
            if commented {
                if name.eq_ignore_ascii_case("multilib") {
                    view.has_commented_multilib = true;
                    view.commented_multilib_source
                        .get_or_insert_with(|| path.to_path_buf());
                }
            } else {
                if !name.eq_ignore_ascii_case("options") {
                    view.repositories.push(RepoEntry {
                        name: name.clone(),
                        enabled: true,
                        source_file: path.to_path_buf(),
                        line: index + 1,
                    });
                }
                section = Some(name);
            }
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if key.eq_ignore_ascii_case("Include") {
            if let Some(include) = resolve_include(value, path) {
                if !view.includes.contains(&include) {
                    view.includes.push(include.clone());
                    let _ = inspect_file(&include, view, depth + 1);
                }
            }
            continue;
        }

        if section
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("options"))
            && key.eq_ignore_ascii_case("ParallelDownloads")
        {
            if let Ok(value) = value.parse::<u32>() {
                view.parallel_downloads = Some(value);
                view.parallel_downloads_source = Some(path.to_path_buf());
            }
        }
    }

    Ok(())
}

fn section_header(line: &str) -> Option<(String, bool)> {
    let trimmed = line.trim_start();
    let (commented, rest) = match trimmed.strip_prefix('#') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, trimmed),
    };
    let rest = rest.split('#').next().unwrap_or(rest).trim();
    let inner = rest.strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner.trim();
    if inner.is_empty() {
        None
    } else {
        Some((inner.to_string(), commented))
    }
}

fn resolve_include(value: &str, source: &Path) -> Option<PathBuf> {
    let value = value.split('#').next().unwrap_or(value).trim();
    if value.is_empty() || value.contains(['*', '?', '[']) {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Some(path)
    } else {
        source.parent().map(|parent| parent.join(path))
    }
}

/// Uncomment the canonical `[multilib]` block, or append one when the file has
/// no block at all.
pub fn enable_multilib_text(text: &str) -> Result<String, String> {
    if enabled_section_exists(text, "multilib") {
        return Err("The multilib repository is already enabled.".to_string());
    }

    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    if let Some(header) = lines.iter().position(|line| {
        matches!(
            section_header(line.trim()),
            Some((name, true)) if name.eq_ignore_ascii_case("multilib")
        )
    }) {
        lines[header] = uncomment(&lines[header]);
        let mut index = header + 1;
        while index < lines.len() {
            let trimmed = lines[index].trim();
            if trimmed.is_empty() {
                break;
            }
            let Some(rest) = trimmed.strip_prefix('#') else {
                break;
            };
            let rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            let is_directive = rest
                .split_once('=')
                .map(|(key, _)| {
                    matches!(
                        key.trim().to_ascii_lowercase().as_str(),
                        "include" | "server" | "siglevel" | "usage"
                    )
                })
                .unwrap_or(false);
            if !is_directive {
                break;
            }
            lines[index] = uncomment(&lines[index]);
            index += 1;
        }
        return Ok(finish_lines(lines));
    }

    let mut output = text.trim_end().to_string();
    output.push_str("\n\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n");
    Ok(output)
}

/// Comment out an existing `[multilib]` section in a file Skemer owns.
pub fn disable_multilib_text(text: &str) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    let Some(header) = lines.iter().position(|line| {
        matches!(
            section_header(line.trim()),
            Some((name, false)) if name.eq_ignore_ascii_case("multilib")
        )
    }) else {
        return Err("The multilib repository is not defined in /etc/pacman.conf.".to_string());
    };

    lines[header] = comment(&lines[header]);
    let mut index = header + 1;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.starts_with('[') {
            break;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            lines[index] = comment(&lines[index]);
        }
        index += 1;
    }
    Ok(finish_lines(lines))
}

/// Set `ParallelDownloads` inside the main `[options]` section.
pub fn set_parallel_downloads_text(text: &str, value: u32) -> Result<String, String> {
    if !(1..=10).contains(&value) {
        return Err("Parallel downloads must be between 1 and 10.".to_string());
    }
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
    let Some(header) = lines.iter().position(|line| {
        matches!(
            section_header(line.trim()),
            Some((name, false)) if name.eq_ignore_ascii_case("options")
        )
    }) else {
        return Err("The [options] section is missing from /etc/pacman.conf.".to_string());
    };

    let mut index = header + 1;
    let mut replaced = false;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.starts_with('[') {
            break;
        }
        if trimmed
            .split_once('=')
            .map(|(key, _)| key.trim().eq_ignore_ascii_case("ParallelDownloads"))
            .unwrap_or(false)
        {
            lines[index] = format!("ParallelDownloads = {value}");
            replaced = true;
            break;
        }
        index += 1;
    }
    if !replaced {
        lines.insert(header + 1, format!("ParallelDownloads = {value}"));
    }
    Ok(finish_lines(lines))
}

fn enabled_section_exists(text: &str, name: &str) -> bool {
    text.lines().any(|line| {
        matches!(
            section_header(line.trim()),
            Some((found, false)) if found.eq_ignore_ascii_case(name)
        )
    })
}

fn uncomment(line: &str) -> String {
    match line.trim_start().strip_prefix('#') {
        Some(rest) => {
            let indent = &line[..line.len() - line.trim_start().len()];
            format!("{indent}{}", rest.trim_start())
        }
        None => line.to_string(),
    }
}

fn comment(line: &str) -> String {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return line.to_string();
    }
    let indent = &line[..line.len() - trimmed.len()];
    format!("{indent}#{trimmed}")
}

fn finish_lines(lines: Vec<String>) -> String {
    let mut output = lines.join("\n");
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "\
[options]
HoldPkg = pacman glibc
ParallelDownloads = 5

[core]
Include = /etc/pacman.d/mirrorlist

[extra]
Include = /etc/pacman.d/mirrorlist

# If you want to run 32 bit applications on your x86_64 system,
# enable the multilib repositories as required here.

#[multilib-testing]
#Include = /etc/pacman.d/mirrorlist

#[multilib]
#Include = /etc/pacman.d/mirrorlist

#[custom]
#Server = file:///home/custompkgs
";

    #[test]
    fn parses_repositories_and_options() {
        let directory = tempdir("parse");
        let main = directory.join("etc/pacman.conf");
        fs::write(&main, DEFAULT).unwrap();
        let view = inspect(&directory).unwrap();
        assert_eq!(
            view.repositories
                .iter()
                .map(|repo| repo.name.as_str())
                .collect::<Vec<_>>(),
            vec!["core", "extra"]
        );
        assert!(view.has_commented_multilib);
        assert_eq!(view.parallel_downloads, Some(5));
        assert!(!view.pacnew);
        fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn tracks_repository_provenance_through_includes() {
        let directory = tempdir("include");
        fs::create_dir_all(directory.join("etc/pacman.conf.d")).unwrap();
        fs::write(
            directory.join("etc/pacman.conf"),
            "[options]\n[core]\nInclude = /etc/pacman.d/mirrorlist\n\nInclude = pacman.conf.d/extra.conf\n",
        )
        .unwrap();
        fs::write(
            directory.join("etc/pacman.conf.d/extra.conf"),
            "[multilib]\nInclude = /etc/pacman.d/mirrorlist\n",
        )
        .unwrap();
        let view = inspect(&directory).unwrap();
        let multilib = view.multilib().expect("multilib");
        assert_eq!(
            multilib.source_file,
            directory.join("etc/pacman.conf.d/extra.conf")
        );
        assert!(!view.has_commented_multilib);
        fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn enables_a_commented_block_in_place() {
        let updated = enable_multilib_text(DEFAULT).unwrap();
        assert!(updated.contains("\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n"));
        assert!(!updated.contains("#[multilib]\n"));
        assert!(updated.contains("#[multilib-testing]"));
        assert!(enabled_section_exists(&updated, "multilib"));
    }

    #[test]
    fn appends_a_block_when_none_exists() {
        let updated = enable_multilib_text("[options]\n[core]\n").unwrap();
        assert!(updated.ends_with("[multilib]\nInclude = /etc/pacman.d/mirrorlist\n"));
    }

    #[test]
    fn refuses_to_enable_twice() {
        let updated = enable_multilib_text(DEFAULT).unwrap();
        assert!(enable_multilib_text(&updated).is_err());
    }

    #[test]
    fn disables_only_the_exact_section() {
        let updated = enable_multilib_text(DEFAULT).unwrap();
        let disabled = disable_multilib_text(&updated).unwrap();
        assert!(!enabled_section_exists(&disabled, "multilib"));
        assert!(disabled.contains("#[multilib]"));
        assert!(disabled.contains("#[multilib-testing]"));
    }

    #[test]
    fn sets_parallel_downloads_in_options() {
        let updated = set_parallel_downloads_text(DEFAULT, 10).unwrap();
        assert!(updated.contains("ParallelDownloads = 10"));
        assert_eq!(updated.matches("ParallelDownloads").count(), 1);
    }

    #[test]
    fn rejects_out_of_range_parallel_downloads() {
        assert!(set_parallel_downloads_text(DEFAULT, 0).is_err());
        assert!(set_parallel_downloads_text(DEFAULT, 11).is_err());
    }

    fn tempdir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "skemer-pacman-config-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("etc")).unwrap();
        directory
    }
}
