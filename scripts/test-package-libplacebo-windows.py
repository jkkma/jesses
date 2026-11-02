"""Windows system import libraries must remain in FFmpeg's linker flags."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("windows_processing", Path(__file__).with_name("build-package-libplacebo-windows.py"))
processing = importlib.util.module_from_spec(spec)
spec.loader.exec_module(processing)


class WindowsProcessingMetadataTests(unittest.TestCase):
    def test_shlwapi_import_archive_uses_a_linker_library_flag(self):
        with tempfile.TemporaryDirectory() as temporary:
            compiler = Path(temporary) / "compiler"
            metadata = Path(temporary) / "libplacebo.pc"
            original = f"Libs: -L${{libdir}} -lplacebo -lm {(compiler / 'ucrt64/lib/libshlwapi.a').as_posix()} -lglslang\nLibs.private: -lstdc++\n"
            metadata.write_text(original)
            processing.complete_windows_static_metadata(metadata, compiler)
            expected = "Libs: -L${libdir} -lplacebo -lm -lshlwapi -lglslang\nLibs.private: -lstdc++\n"
            self.assertEqual(metadata.read_text(), expected)
            processing.complete_windows_static_metadata(metadata, compiler)
            self.assertEqual(metadata.read_text(), expected)


if __name__ == "__main__":
    unittest.main()
