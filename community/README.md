# Community directory

`community.json` is the public list of DeCombat servers and the known-mods list (the "verified" badge). It is
maintained by pull request and served as a static file at one fixed URL, which the client and the website read:

    https://decombat.com/community.json

The client trusts only what it can check from this file: server addresses, and mod hashes and badges. It never
downloads or runs code from it. A mod is "Verified" when the server's `(id, version, sha256)` equals a `known_mods`
entry; anything else is "Unverified" and needs the player's explicit opt-in on first join.

Validate locally (the same check CI runs):

    cargo run -p dc-config -- check-directory community/community.json

## Add a server

Open a pull request that adds one entry to `servers`:

- `id`: unique lowercase slug (3-40 chars of a-z, 0-9, `-`); `name` (at most 48 chars); optional `description`
  (at most 400), `website` (https) and `fingerprint` (the SHA-256 fingerprint the lobby's log prints; the
  in-client browser pins it for the login).
- `region`: one of na, sa, eu, as, oc, af, lan.
- `address`: `host:port` of the lobby, as the client connects.
- `policy`: `vanilla` (no mods, `client_visual_allowed` false) or `modded` (at least one mod).
- `mods`: for a modded server the ordered mod set, copied exactly from `dc-modkit modset <mod folder>` or from the
  lobby's served `/server_list.json`: each item is `id`, `version`, `sha256`.
- Do not put secrets, passwords or private keys in the entry. A contact is the pull request author or `website`.

## Add a known mod

Open a pull request that adds one entry to `known_mods`: `id`, `name`, `version`, `sha256` (of the release zip),
`source_repo` (https URL of the real source repository), plus `author`, `description` and the manifest's
declared `capabilities`. A new version is a new entry; the same `(id, version)` must never carry two hashes.

## What reviewers check

- The release zip attached to the GitHub release hashes to the entry's `sha256`.
- The repository is the real source of that release.
- The capabilities listed match the manifest and the description (a mod with `world.full_visibility` says so).
- For a server: it is reachable, its served mod set equals the entry, and the policy is honest.
- `check-directory` passes (CI fails the build otherwise).

## Publishing

After a merge, copy this file to the website as `community.json` and `data/community.json` and deploy it. The client
reads the first and the website's workshop reads the second.

## The review list

`vanilla_plus.json` is the list of reviewed packs of the Vanilla++ server entry (LG-315); format and steps are in
`docs/MODDING_GUIDE.md` section 12. Check it with `cargo run -p dc-config -- check-vanilla-plus community/vanilla_plus.json`.
