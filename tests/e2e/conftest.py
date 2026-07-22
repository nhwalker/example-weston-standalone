import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

from support.compositor import Westonite  # noqa: E402


@pytest.fixture
def westonite(tmp_path):
    """Factory fixture: launch westonite instances, always torn down.

    Instances started with the factory are SIGTERMed at test end and
    must exit 0 -- every test doubles as a clean-shutdown test. Tests
    that already stopped an instance themselves are skipped by the
    teardown.
    """
    instances = []

    def launch(*, wait=True, **kw):
        w = Westonite(tmp_path / f"w{len(instances)}", **kw)
        instances.append(w)
        if wait:
            w.wait_ready()
        return w

    yield launch

    errors = []
    for w in instances:
        try:
            if w.proc.poll() is None:
                w.terminate()
        except AssertionError as e:
            errors.append(str(e))
        finally:
            w.kill()
    if errors:
        raise AssertionError("teardown failures:\n" + "\n".join(errors))
