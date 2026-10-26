"""Load the optional repository-local x402 debug scope without shell evaluation."""

from pathlib import Path
import os


KEY = "ZETA_X402_DEBUG_BASE_URL"


def environment(repository: Path) -> dict[str, str]:
    """Return the process environment with the one supported `.env` value applied."""
    result = os.environ.copy()
    if result.get(KEY):
        return result
    path = repository / ".env"
    if not path.exists():
        return result
    value = None
    for line in path.read_text(encoding="utf-8").splitlines():
        candidate = line.strip()
        if not candidate or candidate.startswith("#"):
            continue
        if candidate.startswith("export "):
            candidate = candidate.removeprefix("export ").lstrip()
        name, separator, raw = candidate.partition("=")
        if separator and name.strip() == KEY:
            if value is not None:
                raise ValueError(f"{KEY} is duplicated in .env.")
            value = raw.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
                value = value[1:-1]
    if value:
        if any(character.isspace() or character == "\0" for character in value):
            raise ValueError(f"{KEY} in .env contains invalid whitespace.")
        result[KEY] = value
    return result
