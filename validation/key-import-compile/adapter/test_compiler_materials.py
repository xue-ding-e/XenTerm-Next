import unittest
from unittest.mock import Mock
import compiler_materials as m
ROOT=r"C:\Program Files\Microsoft Visual Studio\18\Enterprise"
class Materials(unittest.TestCase):
 def test_fixed_actual_binary_headers_and_libraries(self):
  paths=m.fixed_paths('14.44.35207',ROOT);self.assertEqual(len(paths),10)
  self.assertEqual(str(paths['git-bash']),r'C:\Program Files\Git\bin\bash.exe')
  self.assertTrue(str(paths['cl']).endswith('Hostx64\\x64\\cl.exe'))
  read=Mock(return_value={'bytes':1,'sha256':'a'*64});r=m.capture('14.44.35207','b'*40,123,ROOT,read)
  self.assertEqual(read.call_count,10);self.assertFalse(r['product_environment_qualified']);m.unchanged(r,r.copy())
  with self.assertRaises(ValueError):m.unchanged(r,dict(r,run_id='124'))
 def test_rejects_arbitrary_root_version_or_bad_evidence(self):
  for v in ['../14.44.35207','14.44.35207/other','preview','14.44.35207;cmd']:
   with self.assertRaises(ValueError):m.fixed_paths(v,ROOT)
  for row in [{'bytes':0,'sha256':'a'*64},{'bytes':True,'sha256':'a'*64},{'bytes':1,'sha256':'x'*64}]:
   with self.assertRaises(ValueError):m.capture('14.44.35207','b'*40,123,ROOT,Mock(return_value=row))
if __name__=='__main__':unittest.main()
