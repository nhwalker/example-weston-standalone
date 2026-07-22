import os
import shutil
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

from support.compositor import Westonite  # noqa: E402


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    """On failure, copy the test's working dir (compositor logs, config
    files, stub-client output) into $WESTONITE_E2E_ARTIFACTS so CI can
    upload it. Regular files only -- runtime dirs contain sockets --
    and never let collection problems break the test run itself."""
    outcome = yield
    report = outcome.get_result()
    artifacts = os.environ.get("WESTONITE_E2E_ARTIFACTS")
    if not artifacts or report.when != "call" or not report.failed:
        return
    try:
        tmp_path = item.funcargs.get("tmp_path")
        if not tmp_path or not Path(tmp_path).exists():
            return
        dest = Path(artifacts) / item.name
        for src in Path(tmp_path).rglob("*"):
            if not src.is_file() or src.is_symlink() or src.is_socket():
                continue
            target = dest / src.relative_to(tmp_path)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, target)
    except OSError as exc:
        sys.stderr.write(f"artifact collection failed for {item.name}: {exc}\n")


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
