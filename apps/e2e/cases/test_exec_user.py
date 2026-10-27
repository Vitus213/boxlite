"""Per-exec `user` over the hosted REST path (#2116): SDK -> API -> Runner -> guest."""
from __future__ import annotations

import asyncio

import boxlite
import pytest

from conftest import drain

IDS = ["-c", "echo $(id -u):$(id -g)"]
MARKER = "/tmp/marker-2116"


async def _run(box, args, user=None):
    ex = await box.exec("sh", args, user=user)
    out, _ = await drain(ex)
    rc = await asyncio.wait_for(ex.wait(), timeout=30)
    return rc.exit_code, out.strip()


@pytest.mark.asyncio
async def test_exec_user_runs_as_requested_ids(box):  # cases 1, 4
    for spec, expected in [("0:0", "0:0"), ("1000:1000", "1000:1000"), ("9999", "9999:0")]:
        assert await _run(box, IDS, spec) == (0, expected), spec


@pytest.mark.asyncio
async def test_exec_user_permissions_and_box_default(rt, box, image):  # cases 2, 3
    assert (await _run(box, ["-c", "touch /root/x-2116"], "1000:1000"))[0] != 0
    assert (await _run(box, ["-c", "touch /root/x-2116"], "0:0"))[0] == 0
    baseline = await _run(box, IDS)
    assert await _run(box, IDS, "1000:1000") == (0, "1000:1000")
    assert await _run(box, IDS) == baseline
    b = await rt.create(boxlite.BoxOptions(image=image, user="1000:1000", auto_remove=True))
    try:
        assert await _run(b, IDS) == (0, "1000:1000")
        assert await _run(b, IDS, "0:0") == (0, "0:0")
        assert await _run(b, IDS) == (0, "1000:1000")
    finally:
        await rt.remove(b.id, force=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("spec", ["nosuchuser", "1000:nosuchgroup", ":1000"])
async def test_exec_user_invalid_is_rejected(box, spec):  # case 5
    with pytest.raises(boxlite.InvalidArgumentError):
        await _run(box, ["-c", f"touch {MARKER}"], spec)
    assert (await _run(box, ["-c", f"test -e {MARKER}"]))[0] == 1
