"""Compose and reverify a cached runtime from two pinned development payloads."""
import json
from pathlib import Path
import shutil
import subprocess


def stage(artifact, bundled, output, digest, verify):
    """Caller verifies the experimental pin; this verifies every added MLX file."""
    manifest = json.loads((artifact/'artifact.json').read_text())
    payload = bundled/'payload.sha256'
    expected = {}
    for line in payload.read_text().splitlines():
        checksum, name = line.split(maxsplit=1)
        name = name.removeprefix('./')
        if name == 'ollama' or name.startswith(('mlx_metal_v3/', 'mlx_metal_v4/')):
            path = Path(name)
            if path.is_absolute() or '..' in path.parts or name in expected:
                raise ValueError('invalid bundled MLX payload path')
            expected[name] = checksum
    required = {'ollama', 'mlx_metal_v3/libmlx.dylib', 'mlx_metal_v3/libmlxc.dylib',
                'mlx_metal_v3/mlx.metallib', 'mlx_metal_v4/libmlx.dylib',
                'mlx_metal_v4/libmlxc.dylib', 'mlx_metal_v4/mlx.metallib'}
    if not required <= expected.keys() or expected['ollama'] != manifest['files']['ollama']:
        raise ValueError('MLX composition requires the same pinned Ollama executable')
    for name, checksum in expected.items():
        source = bundled/name
        if (source.is_symlink() or not source.is_file()
                or not source.resolve().is_relative_to(bundled.resolve())
                or digest(source) != checksum):
            raise ValueError(f'bundled MLX integrity failure: {name}')
    for name, checksum in expected.items():
        if name == 'ollama':
            continue
        if name in manifest['files']:
            raise ValueError('experimental artifact already owns an MLX payload')
        manifest['files'][name] = checksum
    manifest['composition'] = {
        'kind': 'zeta-development-mlx-composition-v1',
        'experimental_artifact_sha256': digest(artifact/'artifact.json'),
        'bundled_payload_sha256': digest(payload),
        'mlx_tuning': 'upstream',
    }
    destination = output/'runtime'
    if output.exists():
        verify(destination)
        if json.loads((destination/'artifact.json').read_text()) != manifest:
            raise ValueError('cached runtime differs from its pinned composition')
        return destination
    output.mkdir(parents=True, exist_ok=False)
    shutil.copytree(artifact, destination)
    for name in expected:
        if name != 'ollama':
            target = destination/name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(bundled/name, target)
    (destination/'artifact.json').write_text(json.dumps(manifest, indent=2)+'\n')
    verify(destination)
    return destination


def prepare_loader(runtime):
    # On first use macOS can validate the copied MLX dylibs before Ollama reaches
    # main. Complete that bounded initialization before the launcher's 30s check.
    print('Checking MLX runtime libraries (first use can take a few minutes)...', flush=True)
    try:
        subprocess.run([str(runtime/'ollama'), '--help'], stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, timeout=300, check=True)
    except (subprocess.SubprocessError, OSError) as error:
        raise ValueError('MLX runtime library initialization failed') from error
