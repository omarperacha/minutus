"""Closed role/pin records for native worker transport, never caller policy."""
from dataclasses import dataclass
import re

MAGIC = "minutus-worker-brokers-v1"


@dataclass(frozen=True)
class Role:
    name: str
    worker: str
    memory: int
    wall: int
    file_limit: int

    @property
    def service(self):
        return f"ai.minutus.worker.{self.name}"

    @property
    def executable(self):
        return f"minutus-{self.name}-broker"


ROLES = (
    Role("compiler", "minutus-solidity-compiler", 2 * 1024**3, 60, 16 * 1024**2),
    Role("evm-online", "minutus-evm-simulator", 256 * 1024**2, 15, 256 * 1024**2),
    Role("base-offline", "minutus-base-offline-simulator", 256 * 1024**2, 15, 256 * 1024**2),
)


def parse(text):
    """Require every fixed role once, in build order, with canonical exact pins."""
    if len(text.encode("ascii")) > 4096 or not text.endswith("\n"):
        raise ValueError("Invalid worker broker manifest size or terminator")
    lines = text.splitlines()
    if len(lines) != len(ROLES) + 1 or lines[0] != MAGIC:
        raise ValueError("Invalid worker broker manifest shape")
    records = []
    for role, line in zip(ROLES, lines[1:]):
        fields = line.split(" ")
        if (len(fields) != 8 or fields[:3] != [role.name, role.service, role.executable]
                or fields[5] != role.worker
                or any(re.fullmatch(r"[0-9a-f]{64}", fields[index]) is None for index in (3, 6))
                or any(re.fullmatch(r"[0-9a-f]{40}", fields[index]) is None for index in (4, 7))):
            raise ValueError("Invalid or substituted worker broker role/pin")
        records.append((role, fields))
    return records
