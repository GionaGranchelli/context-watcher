"""Only build/toolchain paths and temporary-directory settings reach offline children."""
import os


def build_environment(source=None):
    source = os.environ if source is None else source
    allowed = ('PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME', 'RUSTUP_TOOLCHAIN',
               'TMPDIR', 'TMP', 'TEMP', 'SYSTEMROOT')
    result = {key: source[key] for key in allowed if key in source}
    result.update(LANG='C.UTF-8', LC_ALL='C.UTF-8', PYTHONDONTWRITEBYTECODE='1')
    return result
