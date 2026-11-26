"""Check that Windows shader pre-build initializes a fresh pipeline cache.

From the repository root after building victory:
    py -3 tests/check_cold_cache.py target/release/victory.exe
"""

import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("victory", type=Path, help="path to the Windows victory executable")
    args = parser.parse_args()
    victory = args.victory.resolve()
    if not victory.is_file():
        parser.error(f"executable does not exist: {victory}")

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        inputs = root / "empty-input"
        inputs.mkdir()
        cache = root / "cache"
        env = os.environ.copy()
        env.pop("VICTORY_CONFIG", None)
        env["VICTORY_CACHE_DIR"] = str(cache)
        env["VICTORY_DATA_DIR"] = str(root / "data")
        subprocess.run(
            [str(victory), "translate", "--cache", "--pipelines", str(inputs)],
            cwd=root, env=env, check=True, timeout=60,
        )
        if not (cache / "pipelines").is_dir():
            raise SystemExit("FAIL: pre-build returned without creating cache/pipelines")
    print("PASS: pre-build created cache/pipelines on a cold start")


if __name__ == "__main__":
    main()
