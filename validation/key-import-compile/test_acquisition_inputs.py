import tempfile,unittest
from unittest.mock import patch
from pathlib import Path
import acquisition_inputs as a
class Inputs(unittest.TestCase):
 def test_empty_isolated_inputs_and_budget(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);(r/'source').mkdir();(r/'cargo-home').mkdir();(r/'home').mkdir()
   self.assertGreater(a.reject_unreviewed_config(r)['reviewed_absent_configuration_paths'],0)
   self.assertEqual(a.inspect_cache_budget(r,a.MIN_FREE_BYTES)['bytes'],0)
   (r/'cargo-home/config.toml').write_text('synthetic not read')
   with self.assertRaises(ValueError):a.reject_unreviewed_config(r)
 def test_symlink_configuration_and_low_disk_rejected(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);(r/'source').mkdir();(r/'cargo-home').mkdir();(r/'home').mkdir()
   (r/'home/.netrc').symlink_to(r/'absent')
   with self.assertRaises(ValueError):a.reject_unreviewed_config(r)
   with self.assertRaises(ValueError):a.inspect_cache_budget(r,a.MIN_FREE_BYTES-1)

 def test_unreadable_cache_and_non_directory_fail_closed(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);(r/'cargo-home').write_text('not a directory')
   with self.assertRaisesRegex(ValueError,'cache directory required'):a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
   (r/'cargo-home').unlink();(r/'cargo-home').mkdir()
   with patch.object(a.os,'scandir',side_effect=PermissionError('synthetic denied')),self.assertRaises(PermissionError):
    a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
 def test_scan_checkpoint_interrupts_with_bounded_entries(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);cache=r/'cargo-home';cache.mkdir()
   for i in range(130):(cache/str(i)).write_bytes(b'x')
   calls=[]
   def checkpoint():
    calls.append(True)
    if len(calls)==2:raise TimeoutError('synthetic deadline')
   with self.assertRaisesRegex(TimeoutError,'deadline'):a.inspect_cache_budget(r,a.MIN_FREE_BYTES,checkpoint)
   self.assertEqual(len(calls),2)
 def test_directory_only_growth_is_bounded(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);cache=r/'cargo-home';cache.mkdir()
   for i in range(3):(cache/str(i)).mkdir()
   with patch.object(a,'MAX_CACHE_FILES',2),self.assertRaisesRegex(ValueError,'directory budget'):
    a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
 def test_known_installed_shims_not_followed_but_cache_reparse_rejected(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);cache=r/'cargo-home';cache.mkdir();tools=cache/'bin';tools.mkdir()
   (tools/'cargo.exe').symlink_to(r/'unqueried-target')
   result=a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
   self.assertTrue(result['cache_excludes_installed_bin']);self.assertEqual(result['files'],0)
   registry=cache/'registry';registry.mkdir();(registry/'bad').symlink_to(r/'unqueried-target')
   with self.assertRaisesRegex(ValueError,'reparse'):a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
 def test_installed_bin_directory_itself_must_not_be_reparse(self):
  with tempfile.TemporaryDirectory() as d:
   r=Path(d);cache=r/'cargo-home';cache.mkdir();target=r/'target';target.mkdir()
   (cache/'bin').symlink_to(target,target_is_directory=True)
   with self.assertRaisesRegex(ValueError,'reparse'):a.inspect_cache_budget(r,a.MIN_FREE_BYTES)
if __name__=='__main__':unittest.main()
