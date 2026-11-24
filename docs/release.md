# Release Runbook

Skemer ships prebuilt binaries as signed GitHub releases. Nothing is compiled on
the release machine: GitHub Actions builds in an Arch container, signs the
artifacts with the Skemer release key, and publishes them. Veldmuis then
verifies the release against the vendored public key before repackaging it as
`veldmuis-skemer`.

This document covers the operational side. For the tarball layout, dependencies,
and package contents see [`../packaging/README.md`](../packaging/README.md); for
key creation see [`../packaging/keys/README.md`](../packaging/keys/README.md).

## What a release contains

```text
skemer-<version>-x86_64-linux.tar.gz        build output
skemer-<version>-x86_64-linux.tar.gz.asc    detached signature
SHA256SUMS                                  aggregate checksum
SHA256SUMS.asc                              detached signature
skemer-release.asc                          public key of the signer
```

The tarball is built from a clean checkout of the tag. The version comes from
`crates/skemer-shell/tauri.conf.json`, and the workflow rejects a tag that does
not equal `v<version>`.

## Prerequisites

### Public key material

These files are committed and must be present on the tag commit:

```text
packaging/keys/skemer-release.gpg
packaging/keys/skemer-release.asc
packaging/keys/skemer-trusted
```

Current key:

```text
primary fingerprint  7C1DFB8E8D0B7FE5744034DE028649D05074D1B7
signing subkey       A0B9977A29AD452430B90D56E642A67A5995E10E
uid                  Skemer release signing <skemer@veldmuislinux.org>
```

The release job stops with a clear message if any of the three files is missing.

### `release` environment secrets

The publish job runs in the protected `release` environment, which provides:

```text
SKEMER_GPG_PRIVATE_KEY   armored signing subkey
SKEMER_GPG_PASSPHRASE    passphrase for that subkey
SKEMER_GPG_FPR           primary fingerprint, must match skemer-trusted
```

Values are write-only. Confirm the names with:

```sh
gh secret list --env release --repo bornmanr/skemer
```

### Key custody

- The **primary key is certify-only** and never leaves offline storage.
- Releases are signed by a **sign-only subkey** whose armored export is the
  `SKEMER_GPG_PRIVATE_KEY` secret.
- Offline storage holds a zip with the full secret
  (`skemer-release-full-secret.asc`) and the revocation certificate
  (`skemer-release-revoke.asc`).
- The **passphrase is stored separately** from that zip and from any copy of the
  signing subkey. Keeping the passphrase beside the subkey export turns one file
  into full signing capability.
- Never commit private keys, the passphrase, or the revocation certificate.

`packaging/setup-release-key.sh` builds this layout in one pass: isolated
keyring, operator-supplied passphrase, full-secret and revocation backup, a zip
of those two files only, the GitHub secrets, and the public files copied into
both checkouts. It verifies that the full secret imports and that the subkey
signs before it writes the zip.

## Cut a release

Patch releases are cut automatically; see [Daily releases](#daily-releases).
Use this manual path for a deliberate minor or major bump.

1. Bump the version in `crates/skemer-shell/tauri.conf.json` and keep the
   workspace version in `Cargo.toml` in step.
2. Merge the version bump to `main` and confirm `packaging/keys/` is present.
3. Tag and push from a checkout that has the merged `main`:

   ```sh
   git tag -a v0.1.1 -m "Skemer 0.1.1"
   git push origin v0.1.1
   ```

4. Watch the `release` workflow. It runs three jobs:

   | Job | What it does |
   | --- | --- |
   | Resolve version | reads `tauri.conf.json`, refuses a mismatched tag |
   | Linux tarball | Arch container, libalpm ABI gate, frontend and binary build, tarball |
   | Publish GitHub Release | key preflight, `SHA256SUMS`, signs, verifies, publishes |

5. Confirm the release is published rather than drafted, and that all five
   assets are attached.

`workflow_dispatch` runs the build jobs without publishing; only a `v*` tag
creates a release.

## Daily releases

`Daily Release` runs at 12:00 Africa/Johannesburg (10:00 UTC) and can also be
dispatched manually from `main`.

- If `main` has no commits since the latest `vX.Y.Z` release, it exits without
  doing anything.
- If the latest tag has no published release, it retries that release before
  bumping anything.
- Otherwise it bumps the patch version, or uses a higher version already
  declared in `tauri.conf.json` for a deliberate minor or major bump.
- It updates `Cargo.toml`, `Cargo.lock`, `tauri.conf.json`, and the frontend
  package files, commits `chore: release X.Y.Z`, pushes `main`, tags `vX.Y.Z`,
  and calls the reusable `release` workflow for that commit.
- The tag push uses `GITHUB_TOKEN`, which does not start another workflow run,
  so the daily workflow calls `release` directly instead of relying on the tag
  trigger. Manual tags still trigger `release` normally.

The bot commits to `main`. If branch protection is added, allow the Actions bot
to push, or change the workflow to open a bump pull request. The daily commit
does not trigger CI; the release build is its verification gate.

To cut the next release immediately, merge any pending changes and run
**Actions → Daily Release → Run workflow** from `main`.

## Verify a release

Any machine with `gpgv` can check a release against the committed public key:

```sh
gh release download v0.1.1 --repo bornmanr/skemer \
  --pattern 'SHA256SUMS*' --pattern 'skemer-0.1.1-x86_64-linux.tar.gz*'

gpgv --keyring packaging/keys/skemer-release.gpg SHA256SUMS.asc SHA256SUMS
gpgv --keyring packaging/keys/skemer-release.gpg \
  skemer-0.1.1-x86_64-linux.tar.gz.asc skemer-0.1.1-x86_64-linux.tar.gz
sha256sum -c SHA256SUMS
```

Expect `Good signature from "Skemer release signing
<skemer@veldmuislinux.org>"` twice and `OK` from `sha256sum`.

The publisher runs the same `gpgv` check against the committed keyring before it
creates the release, so a release that exists has already been verified with the
key that consumers vendor.

## Rotate the key

### New signing subkey, same primary

Routine rotation. The fingerprint does not change, so consumers keep the same
pin.

1. Create a sign-only subkey on the offline primary.
2. Update the `SKEMER_GPG_PRIVATE_KEY` and `SKEMER_GPG_PASSPHRASE` secrets.
3. Re-export `skemer-release.gpg` and `.asc` with the new subkey and commit them
   in Skemer, then refresh `development/skemer-keyring/` in Veldmuis.
4. Tag the next release and verify it.

### New primary key

Staged and reviewed, because it changes the pin.

1. Generate the new key and set the three environment secrets.
2. Commit the new public files in Skemer.
3. Open the Veldmuis keyring PR so `development/skemer-keyring/` matches.
4. Merge both before tagging; a tag that still carries the old keyring will
   verify against the old key and Veldmuis will refuse the pin.

### Compromise

1. Stop signing.
2. Revoke from the offline primary using the revocation certificate.
3. Publish an authenticated notice, then rotate to a new primary.

Revoking is why the revocation certificate is stored offline next to the full
secret, not only on the machine that signs.

## libalpm ABI

Skemer links against `libalpm` from the Arch container, so the ABI must match the
Veldmuis host. `packaging/libalpm-soname` pins it (currently `16`).

- The release workflow reads `pacman`'s `Provides` in the build container and
  fails when it does not match the pin.
- The `libalpm ABI Watch` workflow checks daily and opens one issue when Arch
  moves to a new soname.

When the soname changes:

1. Update `packaging/libalpm-soname`.
2. Tag a new release.
3. Update `libalpm.so=<N>-64` in the Veldmuis `veldmuis-skemer` package.

The versioned dependency deliberately blocks a pacman upgrade that would leave
Skemer linked against the old ABI.

## Handoff to Veldmuis

Veldmuis consumes releases through `development/resolve-skemer-release.sh` and
the `Skemer Release Watch` workflow:

- The resolver reads the latest release, requires all four signature assets,
  verifies both signatures against `development/skemer-keyring/`, and only then
  rewrites `pkgver` and `sha256sums` in `packages/veldmuis-skemer/PKGBUILD`.
- The watch workflow runs that resolver on a `chore/skemer-release-pin` branch
  and opens a pull request with `VELDMUIS_SYNC_TOKEN`. Without that secret it
  cannot push or open the PR.
- Merging the pin triggers the package refresh, which rebuilds and publishes
  `veldmuis-skemer` to the Veldmuis repository.

The first signed release (`v0.1.0`, 2026-11-15) also enabled the package in the
Veldmuis manifest, `veldmuis-common`, and the installer base set.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `tag vX does not match tauri.conf.json version` | tag and version disagree | bump the version or move the tag |
| `Missing packaging/keys/...` | key material not on the tag commit | merge the key PR, retag |
| Signing step fails with a passphrase or key error | secret does not match the public key | re-export the subkey and reset the secrets |
| `libalpm ABI changed` in the build | Arch moved the soname | update the pin, tag a release, update the Veldmuis dependency |
| Release is a draft or has no assets | publish job failed after signing | inspect the job log, fix, retag |
| Veldmuis watch reports `HTTP 401` | `VELDMUIS_SYNC_TOKEN` expired | rotate the PAT in the Veldmuis repository settings |
| Veldmuis resolver says no signed release | release lacks the signature assets | publish a release with all four assets |

## Checklist

- [ ] Version bumped in `tauri.conf.json` and `Cargo.toml`
- [ ] `packaging/keys/` present on `main`
- [ ] `release` environment has the three secrets
- [ ] Tag matches `v<version>`
- [ ] Release published with five assets
- [ ] `gpgv` verification passes against `packaging/keys/skemer-release.gpg`
- [ ] Veldmuis watch pinned the new version
