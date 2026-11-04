# Security policy

DeCombat parses untrusted network input (TCP frames, UDP bitstreams) and untrusted community content (mod packages,
Lua scripts, archives). Memory-safety, sandbox-escape, path-traversal, denial-of-service and authentication
bugs are in scope.

## Reporting

Please do not open a public issue for a vulnerability. Use [private vulnerability reporting](https://github.com/argallo/DeCombat/security/advisories/new)
on the repository, or write to contact@decombat.com.
Include the affected crate, version or commit, a minimal reproduction and the impact. We aim to acknowledge
reports within 7 days.

## Scope notes

- The mod sandbox (`dc-mod-lua`) must not give mods file, process or network access beyond declared
  permissions. Sandbox escapes are high severity.
- Run tagged releases on a public server, not development builds. See docs/SERVER.md for the public-server settings.
- Never include real account data, ROM files or keys in a report.

## Supported versions

Only the latest commit on the default branch is supported while the project is pre-release.
