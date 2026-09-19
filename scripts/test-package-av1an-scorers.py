"""Source pin and extraction tests for packaged CPU scorers."""

import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("package_av1an_scorers", SCRIPTS / "package-av1an-scorers.py")
SCORERS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCORERS)


class ScorerSourceTests(unittest.TestCase):
    def test_skcms_commit_root_is_removed_before_staging(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "skcms.tar.gz"
            content = b"pinned header"
            with tarfile.open(archive, "w:gz") as bundle:
                member = tarfile.TarInfo("skcms-b25b07b/skcms.h")
                member.size = len(content)
                bundle.addfile(member, io.BytesIO(content))
            sources = SCORERS.unpack_sources({"skcms": archive}, root / "work")
            self.assertEqual((sources["skcms"] / "skcms.h").read_bytes(), content)
            self.assertFalse((sources["skcms"] / "skcms-b25b07b").exists())


if __name__ == "__main__":
    unittest.main()
