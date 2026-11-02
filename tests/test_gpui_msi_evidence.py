"""Static contract only; native WiX log capture requires Windows CI validation."""
from pathlib import Path
import unittest


class EvidenceContractTests(unittest.TestCase):
    def test_packaging_keeps_only_owned_work_directory(self):
        script = (Path(__file__).resolve().parents[1] / 'scripts/package_next_gpui_msi.ps1').read_text()
        self.assertIn("[string]$EvidenceDir = ''", script)
        self.assertIn("$work = Join-Path $workRoot ('next-gpui-msi-' + [guid]::NewGuid().ToString('N'))", script)
        self.assertIn("'files-before-conversion.wxs'", script)
        for tool, error in [('heat', 'WiX harvest failed'), ('candle', 'WiX compilation failed'),
                            ('light', 'WiX linking/ICE validation failed')]:
            line = next(line for line in script.splitlines() if line.startswith('& "$WixBin/' + tool + '.exe"'))
            self.assertIn(f"Tee-Object -FilePath (Join-Path $work '{tool}.log')", line)
            self.assertIn("if ($LASTEXITCODE -ne 0) { throw '" + error + "' }", script)
        self.assertNotIn('Get-ChildItem $workRoot', script)
        self.assertNotIn('Remove-Item $workRoot', script)


if __name__ == '__main__':
    unittest.main()
