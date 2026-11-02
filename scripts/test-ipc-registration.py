"""Keep native command handlers, dispatch, generated permissions and window access in sync."""
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeCommandRegistration(unittest.TestCase):
    def setUp(self):
        self.source = (ROOT / "src-tauri/src/lib.rs").read_text(encoding="utf-8")
        self.handlers = {
            command.strip()
            for block in re.findall(r"tauri::generate_handler!\[([^\]]+)\]", self.source, re.S)
            for command in block.split(",")
            if re.fullmatch(r"\s*[a-z0-9_]+\s*", command)
        }
        self.assertTrue(self.handlers)

    def test_every_handler_generates_an_application_permission(self):
        build = (ROOT / "src-tauri/build.rs").read_text(encoding="utf-8")
        declared = set(re.findall(r'"([a-z0-9_]+)"', build))
        self.assertEqual(self.handlers - declared, set())

    def test_main_window_grants_every_declared_media_command(self):
        capability = json.loads((ROOT / "src-tauri/capabilities/main.json").read_text(encoding="utf-8"))
        expected = {"allow-" + command.replace("_", "-") for command in self.handlers}
        self.assertEqual(expected - set(capability["permissions"]), set())

    def test_every_handler_has_a_dispatch_route(self):
        router = self.source.split("fn invoke_command(", 1)[1].split("fn ", 1)[0]
        routes = set(re.findall(r'"([a-z0-9_]+)"', router))
        self.assertEqual(self.handlers - routes, set())

    def test_job_handlers_request_a_registered_managed_state(self):
        managed = set(re.findall(r"\bapp\.manage\(\s*([A-Z]\w*)\s*\{", self.source))
        self.assertTrue(managed)
        for command, parameters in re.findall(r"async\s+fn\s+(\w+)\((.*?)\)\s*->", self.source, re.S):
            state = re.search(r"\bjobs:\s*State<\s*'_\s*,\s*(\w+)\s*>", parameters)
            if state and command in self.handlers:
                self.assertIn(state.group(1), managed, f"{command} requests unregistered job state {state.group(1)}")

if __name__ == "__main__":
    unittest.main()
