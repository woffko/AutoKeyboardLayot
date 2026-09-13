import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from revalidate_notice_supplement import revalidate


class RevalidationTests(unittest.TestCase):
    def test_exact_current_revision_content_and_clean_source_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'LICENSE').write_bytes(b'public fixture')
            vcs = root / '.cargo_vcs_info.json'
            vcs.write_text(json.dumps({'git': {'sha1': 'a'*40}}))
            package = {'name': 'fixture', 'version': '1', 'repository': 'https://github.com/example/project',
                       'manifest_path': str(root / 'Cargo.toml')}
            entry = {'package': 'fixture 1', 'repository': 'example/project', 'revision': 'a'*40, 'dirty_source': False,
                     'files': [{'path': 'LICENSE', 'sha256': hashlib.sha256(b'public fixture').hexdigest(),
                                'source_url': 'https://raw.githubusercontent.com/example/project/' + 'a'*40 + '/LICENSE'}]}
            supplement = {'entries': [entry]}
            self.assertEqual(revalidate([package], supplement, {'fixture 1'}, root), [entry])
            for state in [{'sha1': 'b'*40}, {'sha1': 'a'*40, 'dirty': True}]:
                vcs.write_text(json.dumps({'git': state}))
                with self.assertRaises(ValueError):
                    revalidate([package], supplement, {'fixture 1'}, root)
            vcs.write_text(json.dumps({'git': {'sha1': 'a'*40}}))
            (root / 'LICENSE').write_bytes(b'changed')
            with self.assertRaises(ValueError):
                revalidate([package], supplement, {'fixture 1'}, root)
            with self.assertRaises(ValueError):
                revalidate([package, package], supplement, {'fixture 1'}, root)
            with self.assertRaises(ValueError):
                revalidate([package], supplement, {'absent 1'}, root)


if __name__ == '__main__':
    unittest.main()
