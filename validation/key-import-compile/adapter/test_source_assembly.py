import os,tempfile,unittest
from pathlib import Path
from unittest.mock import Mock,patch
import source_assembly as s
class Assembly(unittest.TestCase):
 def test_wrong_source_stops_before_apply(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);git=root/'git';git.write_text('not executed');source=root/'source';source.mkdir();(source/'.git').mkdir();(source/'.git/config').write_text('')
   run=Mock(side_effect=['core.bare\ncore.filemode\ncore.ignorecase\ncore.symlinks','0'*40])
   with self.assertRaisesRegex(ValueError,'main baseline'):
    s.assemble(source,git,{'GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':os.devnull,'GIT_TERMINAL_PROMPT':'0'},run)
   self.assertEqual(run.call_count,2)
 def test_rejects_credential_config_before_git(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);git=root/'git';git.write_text('not executed');run=Mock()
   with self.assertRaisesRegex(ValueError,'Git environment inventory'):
    s.assemble(root,git,{'GH_TOKEN':'synthetic-not-secret'},run)
   run.assert_not_called()

 def test_local_filter_or_include_is_rejected_before_apply(self):
  with tempfile.TemporaryDirectory() as d:
   root=Path(d);git=root/'git';git.write_text('not executed');(root/'.git').mkdir();(root/'.git/config').write_text('')
   for key in ['include.path','filter.evil.clean','core.sshcommand','extensions.worktreeconfig']:
    run=Mock(return_value=key)
    with self.assertRaisesRegex(ValueError,'unapproved local Git configuration'):
     s.assemble(root,git,{'GIT_CONFIG_NOSYSTEM':'1','GIT_CONFIG_GLOBAL':os.devnull,'GIT_TERMINAL_PROMPT':'0'},run)
    self.assertEqual(run.call_count,1)
if __name__=='__main__':unittest.main()
