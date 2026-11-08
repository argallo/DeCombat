# FAQ

## How do I get the 2009 client?

DeCombat includes nothing from the game and links to no downloads. You need your own copy of the 2009 Combat Arms EU client
folder, the one with `Engine.exe` and a `Game` folder, about 3.2 GB. You might have one on an old drive or install disc. It's
up to you to make sure you are allowed to have and use your copy where you live. Please don't ask for a copy in this project's
issues or on its servers, and don't post links to one.

DeCombat checks the folder against the hashes in [CLEANROOM.md](../CLEANROOM.md) and tells you if it's a different build. Only
that build works. Your folder is read, never changed, and its programs are never run.

## Do I need a Nexon account?

No. DeCombat doesn't talk to any Nexon service. Your account is on the server you play on, or on the identity server that
server trusts.

## Is this an official project?

No. See [NOTICE.md](../NOTICE.md).

## How do I play with bots?

[BOTS.md](BOTS.md) shows how to fill a room with bots on your own machine or your own server.

## What is the difference between Vanilla, Vanilla++ and modded servers?

Vanilla is the 2009 game with no mods. Vanilla++ adds reviewed maps, modes and weapons and shares progress with vanilla. A
modded server runs whatever its operator chooses, and your client asks you before it downloads a mod nobody has reviewed. Mods
run in a sandbox and can only do what their manifest declares ([MODDING_GUIDE.md](MODDING_GUIDE.md) section 5).

## Can I make a mod or a map?

Yes. Start with [MODDING_GUIDE.md](MODDING_GUIDE.md) for mods and [MAPKIT_AGENTS.md](MAPKIT_AGENTS.md) for maps. Nothing from the
game may go into a pack.

To get a pack onto Vanilla++, see [section 12 of the guide](MODDING_GUIDE.md#12-making-content-for-vanilla). Anyone can run any
mod on their own modded server with no review.

## Can I run my own server?

Yes. Start with [SERVER.md](SERVER.md). You need the client folder or a smaller server ROM ([SERVER_ROM.md](SERVER_ROM.md)).

## The game crashes a moment after it starts

Most often it is another program's overlay. Windows loads every registered Vulkan overlay layer into the game: the one
of **FPS Monitor** (`fpsmonvk64.dll`) crashes the game at its first frames on the Vulkan renderer. Since 2026-11-08 the
game keeps that layer out by itself and says so in `%LOCALAPPDATA%\DeCombat\logs\dc-client.out` (`Vulkan layer ... disabled`);
with an older build, close or uninstall FPS Monitor, or switch to DirectX 12 (Options, Graphic, Renderer, or a shortcut
with ` --adapter dx12`). A crash report names the module at fault: `crash.txt` in the logs folder, or the diagnostic
report the game sends to the server owner, whose `vulkan_layers` list shows every overlay layer on the machine.

## How do I report a bug?

In a match, press Escape and use the report button in the exit box. It goes to the server you're playing on. For a bug in the
code, [open an issue](https://github.com/argallo/DeCombat/issues) and attach your logs (`%LOCALAPPDATA%\DeCombat\logs` on Windows,
`~/Library/Logs/DeCombat` on macOS). The logs don't contain your password. Remove anything else you'd rather not post, and don't
attach files from the client folder.

Security problems go through [SECURITY.md](../SECURITY.md), not a public issue.

## Who do I contact?

contact@decombat.com. Copyright or trademark requests: see [TAKEDOWN.md](legal/TAKEDOWN.md).
