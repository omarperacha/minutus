# Local EVM worker build pins

`scripts/stage_evm_workers.py` builds and signs the two separate helper binaries,
then generates `manifest.txt` (ignored). The desktop build verifies both SHA256
and code-directory hashes and embeds the exact manifest into the host's immutable
`__TEXT,__evm_workers` Mach-O section. Runtime trust uses that loaded section,
never this resource file or a caller-supplied expected digest.

Stage helpers before packaging. Unpackaged developer hosts cannot obtain trusted
worker capabilities. Every runtime launch rechecks file content and the actual
running child's kernel code identity before releasing request/signature input.

`tauri.evm-workers.conf.json` adds these generated sidecars only to explicit
staging/release packaging. Both packaging scripts stage helpers and apply that
overlay automatically. The base Tauri config permits ordinary unstaged Cargo
checking without generating placeholder executables.
