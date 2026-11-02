"""Compiled hardware encoder exclusion is separate from Vulkan processing."""
import importlib.util
import json
from pathlib import Path
import unittest

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location("cpu_inventory",HERE/"qualify-cpu-encoders.py");POLICY=importlib.util.module_from_spec(spec);spec.loader.exec_module(POLICY)

class CpuEncodingPolicyTests(unittest.TestCase):
    def inventory(self):return "\n".join(" V..... "+name+" fixture" for name in POLICY.POLICY["requiredSoftwareEncoders"])
    def test_every_locked_hardware_family_is_rejected_even_without_a_gpu(self):
        for name in POLICY.POLICY["excludedEncoders"]:
            with self.subTest(encoder=name),self.assertRaises(ValueError):POLICY.inspect_inventory(self.inventory()+"\n V..... "+name+" hardware")
    def test_unknown_new_hardware_encoder_in_known_family_is_rejected(self):
        for suffix in POLICY.POLICY["hardwareEncoderSuffixes"]:
            with self.subTest(family=suffix),self.assertRaises(ValueError):POLICY.inspect_inventory(self.inventory()+"\n V..... future_"+suffix+" hardware")
    def test_required_cpu_inventory_is_not_a_name_only_receipt(self):
        self.assertEqual(set(POLICY.inspect_inventory(self.inventory())),set(POLICY.POLICY["requiredSoftwareEncoders"]))
        with self.assertRaises(ValueError):POLICY.inspect_inventory(self.inventory().replace("libx264","missing"))
    def test_developer_encode_example_exposes_only_cpu_encoder_choices(self):
        text=(HERE.parent/"crates/media-runtime/examples/encode.rs").read_text()
        for name in ("h264Nvenc","hevcNvenc"):
            self.assertNotIn(name,text)
        self.assertIn('Some("x264")',text)
        self.assertIn('Some("av1an")',text)
    def test_encoder_exclusions_preserve_gpu_processing_and_are_pinned_to_source_release(self):
        lock=json.loads((HERE/"package-ffmpeg-linux-lock.json").read_text());self.assertEqual(lock["ffmpeg"]["version"],POLICY.POLICY["ffmpegVersion"])
        for path in [HERE/"build-package-ffmpeg-linux.py",HERE/"build-package-ffmpeg.py",HERE/"ci-install-ffmpeg.py"]:
            text=path.read_text();self.assertIn('"--disable-encoder="',text);self.assertNotIn('"--disable-vulkan"',text);self.assertNotIn('"--disable-libplacebo"',text);self.assertNotIn('"--enable-nvenc"',text)
        self.assertFalse(any(item["id"]=="nv-codec-headers" for item in lock["libraries"]))
        self.assertNotIn("nvCodecHeaders",json.loads((HERE/"package-ffmpeg-windows-lock.json").read_text()))

if __name__=="__main__":unittest.main()
