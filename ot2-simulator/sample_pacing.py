"""Pace a sample run's real protocol engine against recorded command offsets."""

import asyncio
from contextvars import ContextVar

from prepare_replay import timestamp_us


class SamplePacing:
    def __init__(self, commands):
        origin = timestamp_us(commands[0]["startedAt"])
        self._commands = [
            (
                command["commandType"],
                (timestamp_us(command["startedAt"]) - origin) / 1_000_000,
                (timestamp_us(command["completedAt"]) - origin) / 1_000_000,
            )
            for command in commands
        ]
        self._index = 0
        self._origin = None

    async def before_command(self, command_type):
        if self._index >= len(self._commands):
            return None
        expected, start, end = self._commands[self._index]
        if expected != command_type:
            # Leave the protocol engine in control if execution diverges.
            self._index = len(self._commands)
            return None
        self._index += 1
        if self._origin is None:
            self._origin = asyncio.get_running_loop().time()
        await wait_until(self._origin + start)
        return self._origin + end


async def wait_until(deadline):
    if deadline is not None:
        remaining = deadline - asyncio.get_running_loop().time()
        if remaining > 0:
            await asyncio.sleep(remaining)


# The protocol engine queue starts in the run-creation request's context.
sample_pacing = ContextVar("sample_pacing", default=None)
