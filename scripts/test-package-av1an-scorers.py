"""Source pin and extraction tests for packaged CPU scorers."""

import importlib.util
import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SCRIPTS = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("package_av1an_scorers", SCRIPTS / "package-av1an-scorers.py")
SCORERS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCORERS)


class ScorerSourceTests(unittest.TestCase):
    def test_msvc_convolve_patch_changes_only_the_outer_inline_annotation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            path = source / "lib/jxl/convolve_separable5.cc"
            path.parent.mkdir(parents=True)
            original = b"// retained source\nstatic JXL_MAYBE_INLINE void ConvolveRow(\n  int x) { KeepAlgorithm(x); }\nstatic JXL_MAYBE_INLINE void Helper();\n"
            path.write_bytes(original)
            with patch.object(SCORERS, "JXL_CONVOLVE_SHA256", hashlib.sha256(original).hexdigest()):
                SCORERS.patch_msvc_convolve(source, root / "convolve.patch")
            self.assertEqual(path.read_bytes(), original.replace(b"static JXL_MAYBE_INLINE void ConvolveRow(", b"static inline void ConvolveRow("))
            self.assertIn("-static JXL_MAYBE_INLINE void ConvolveRow(", (root / "convolve.patch").read_text())

    def test_msvc_convolve_patch_rejects_changed_pin_before_writing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            path = source / "lib/jxl/convolve_separable5.cc"
            path.parent.mkdir(parents=True)
            original = b"static JXL_MAYBE_INLINE void ConvolveRow() {}\n"
            changed = original + b"// changed\n"
            path.write_bytes(changed)
            with patch.object(SCORERS, "JXL_CONVOLVE_SHA256", hashlib.sha256(original).hexdigest()):
                with self.assertRaisesRegex(ValueError, "pinned.*convolution source"):
                    SCORERS.patch_msvc_convolve(source, root / "convolve.patch")
            self.assertEqual(path.read_bytes(), changed)
            self.assertFalse((root / "convolve.patch").exists())

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
