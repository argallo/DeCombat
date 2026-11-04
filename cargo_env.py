"""Portable Rust + MSVC 2022 environment runner for the Combat Arms Rust workspace.

Configures RUSTUP_HOME, CARGO_HOME (in tools/rust) and MSVC 2022 x86 linker/SDK paths (in tools/msvc2022)
so `cargo build`, `cargo test`, and `cargo run` work deterministically offline inside the sandbox
from any checkout under the folder that holds tools/rust.

Target selection: set DC_TARGET=x86_64 (alias x64) or DC_TARGET=i686 (alias x86, the default for now).
It picks the rustup toolchain and the MSVC Hostx64/{x64,x86} linker and lib directories.
DC_TARGET=aarch64-apple-darwin (alias mac) is check-only: the i686 host toolchain (which carries the
Apple Silicon std) compiles build scripts with the MSVC setup and CARGO_BUILD_TARGET cross-checks the crates.
No Apple SDK exists here, so only `cargo check` works for it, never build/link/test.
DC_TARGET=x86_64-unknown-linux-gnu (alias linux) is check-only the same way (the server binaries for a Linux
VPS): the i686 host toolchain carries the Linux std, the stub C compiler stands in for bundled C code. No Linux
linker or sysroot exists here; build natively on the Linux host with plain cargo (docs/SERVER.md).

Usage:
    python cargo_env.py cargo test
    python cargo_env.py cargo build --release
    DC_TARGET=x86_64 python cargo_env.py cargo test --workspace
    DC_TARGET=linux python cargo_env.py cargo check -p dc-lobby-server -p dc-dserver -p dc-rez
    python cargo_env.py rustup show
"""
import os
import sys
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))


def find_root(start):
    if os.environ.get('COMBATARMS_ROOT'):
        return os.environ['COMBATARMS_ROOT']
    cur = start
    for _ in range(6):
        if os.path.isdir(os.path.join(cur, 'tools', 'rust')):
            return cur
        parent = os.path.dirname(cur)
        if parent == cur:
            break
        cur = parent
    return os.path.dirname(start)


ROOT = find_root(HERE)

RUST_DIR = os.path.join(ROOT, 'tools', 'rust')
RUSTUP_HOME = os.path.join(RUST_DIR, 'rustup')
CARGO_HOME = os.path.join(RUST_DIR, 'cargo')
CARGO_BIN = os.path.join(CARGO_HOME, 'bin')

_TARGET = os.environ.get('DC_TARGET', 'i686').strip().lower()
CROSS_TRIPLE = None
if _TARGET in ('aarch64-apple-darwin', 'mac', 'macos'):
    # Check-only cross target; the host side stays the i686 MSVC toolchain.
    CROSS_TRIPLE = 'aarch64-apple-darwin'
    _TARGET = 'i686'
elif _TARGET in ('x86_64-unknown-linux-gnu', 'linux'):
    # Check-only cross target, like the Mac lane.
    CROSS_TRIPLE = 'x86_64-unknown-linux-gnu'
    _TARGET = 'i686'
if _TARGET in ('i686', 'x86', 'i686-pc-windows-msvc'):
    ARCH, TRIPLE = 'x86', 'i686-pc-windows-msvc'
elif _TARGET in ('x86_64', 'x64', 'amd64', 'x86_64-pc-windows-msvc'):
    ARCH, TRIPLE = 'x64', 'x86_64-pc-windows-msvc'
else:
    sys.exit('cargo_env.py: DC_TARGET must be x86_64, i686, mac or linux, got %r' % _TARGET)
TOOLCHAIN = 'stable-' + TRIPLE
TOOLCHAIN_BIN = os.path.join(RUSTUP_HOME, 'toolchains', TOOLCHAIN, 'bin')

MSVC = os.path.join(ROOT, 'tools', 'msvc2022')
VCV = '14.44.35207'
SDKV = '10.0.26100.0'
VC = os.path.join(MSVC, 'VC', 'Tools', 'MSVC', VCV)
KIT = os.path.join(MSVC, 'Windows Kits', '10')
MSVC_BIN = os.path.join(VC, 'bin', 'Hostx64', ARCH)
SDK_BIN = os.path.join(KIT, 'bin', SDKV, 'x64')
LINKER = os.path.join(MSVC_BIN, 'link.exe')


def write_check_stubs():
    stub = os.path.join(HERE, 'target', 'check-stub')
    os.makedirs(stub, exist_ok=True)
    py = sys.executable
    for name in ('cc', 'ar'):
        with open(os.path.join(stub, name + '.cmd'), 'w') as f:
            f.write('@"%s" "%s" --check-stub %s %%*\n' % (py, os.path.abspath(__file__), name))
    return stub


def check_stub(tool, args):
    # cc: honour `-o <file>`; ar: the first path after the flags argument is the archive.
    out = None
    if tool == 'cc':
        for i, a in enumerate(args):
            if a == '-o' and i + 1 < len(args):
                out = args[i + 1]
            elif a.startswith('-o') and len(a) > 2:
                out = a[2:]
    elif len(args) >= 2:
        out = args[1]
    if out:
        open(out, 'ab').close()
    return 0


def build_env():
    env = dict(os.environ)
    env['RUSTUP_HOME'] = RUSTUP_HOME
    env['CARGO_HOME'] = CARGO_HOME
    path_parts = [TOOLCHAIN_BIN, CARGO_BIN, MSVC_BIN, SDK_BIN, env.get('PATH', '')]
    env['PATH'] = ';'.join(p for p in path_parts if p)
    env['INCLUDE'] = ';'.join([
        os.path.join(VC, 'include'),
        os.path.join(KIT, 'Include', SDKV, 'ucrt'),
        os.path.join(KIT, 'Include', SDKV, 'shared'),
        os.path.join(KIT, 'Include', SDKV, 'um'),
    ])
    env['LIB'] = ';'.join([
        os.path.join(VC, 'lib', ARCH),
        os.path.join(KIT, 'Lib', SDKV, 'ucrt', ARCH),
        os.path.join(KIT, 'Lib', SDKV, 'um', ARCH),
    ])
    env['CARGO_TARGET_%s_LINKER' % TRIPLE.upper().replace('-', '_')] = LINKER
    env['RUSTUP_TOOLCHAIN'] = TOOLCHAIN
    if CROSS_TRIPLE:
        env['CARGO_BUILD_TARGET'] = CROSS_TRIPLE
        # Check-only: C sources of -sys crates (bundled SQLite) cannot be compiled for the cross target here. A stub C
        # compiler and archiver write empty outputs, which is enough for `cargo check` (nothing links).
        stub = write_check_stubs()
        tag = CROSS_TRIPLE.replace('-', '_')
        env['CC_' + tag] = os.path.join(stub, 'cc.cmd')
        env['AR_' + tag] = os.path.join(stub, 'ar.cmd')
    if 'CA_ROM' not in env:
        env['CA_ROM'] = os.path.join(ROOT, 'extracted', 'app', 'client', 'eu-test')
    return env


def main():
    if len(sys.argv) < 2:
        print("Usage: python cargo_env.py <command> [args...]", file=sys.stderr)
        return 2
    if sys.argv[1] == '--check-stub':
        return check_stub(sys.argv[2], sys.argv[3:])
    env = build_env()
    cmd = list(sys.argv[1:])
    if cmd[0] in ('cargo', 'cargo.exe'):
        cmd[0] = os.path.join(TOOLCHAIN_BIN, 'cargo.exe')
    elif cmd[0] in ('rustc', 'rustc.exe'):
        cmd[0] = os.path.join(TOOLCHAIN_BIN, 'rustc.exe')
    elif cmd[0] in ('rustup', 'rustup.exe'):
        cmd[0] = os.path.join(CARGO_BIN, 'rustup.exe')
    res = subprocess.run(cmd, cwd=HERE, env=env)
    return res.returncode


if __name__ == '__main__':
    sys.exit(main())
