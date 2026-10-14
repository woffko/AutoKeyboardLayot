import contextlib
import copy
import io
import json
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import check_catalog_candidate as checker  # noqa: E402

# The public, signed catalog revision 5 (also a seed of the mutation test).
PREVIOUS = TOOLS.parent / 'tests/fixtures/mutation-seed-catalog.aklc'
NOW = 1_791_000_000  # a few days after the previous window ended


def envelope(catalog, signed=False):
    outer = {'format': 1, 'catalog': json.dumps(catalog, separators=(',', ':'))}
    if signed:
        outer.update(signer='test-only', signature='00' * 64)
    return json.dumps(outer)


class CheckerTests(unittest.TestCase):
    def setUp(self):
        self._directory = tempfile.TemporaryDirectory()
        self.addCleanup(self._directory.cleanup)
        self.directory = Path(self._directory.name)
        outer = json.loads(PREVIOUS.read_text(encoding='utf-8'))
        self.previous = json.loads(outer['catalog'])

    def renewal(self, **changes):
        """The next catalog: the same records, revision 6, a 21-day window that started an hour ago."""
        catalog = copy.deepcopy(self.previous)
        catalog.update(revision=6, issued_at=NOW - 3600, expires_at=NOW - 3600 + 21 * 86400)
        catalog.update(changes)
        return catalog

    def write(self, catalog, name='candidate.json', signed=False):
        path = self.directory / name
        path.write_text(envelope(catalog, signed), encoding='utf-8')
        return path

    def check(self, catalog, **options):
        options.setdefault('now', NOW)
        return checker.check(PREVIOUS, self.write(catalog), **options)

    def rejected(self, catalog, fragment, **options):
        with self.assertRaises(checker.Rejected) as caught:
            self.check(catalog, **options)
        self.assertIn(fragment, str(caught.exception))

    def test_the_published_catalog_is_readable_as_the_previous_one(self):
        catalog, signed, digest = checker.read_catalog(PREVIOUS)
        self.assertEqual((catalog['revision'], len(catalog['packages']), signed), (5, 13, True))
        self.assertEqual(digest, 'b0e1d636bd7057e6bdb0393cf77d3f8d1cbf7d9ebffd478ea70d0c340c1ea503')

    def test_a_correct_renewal_passes_whether_signed_or_not(self):
        lines = self.check(self.renewal())
        self.assertIn('revision 6; 13 package records, identical to the previous catalog', lines[2])
        self.assertTrue(lines[0].startswith('candidate: unsigned, sha256 '))
        signed = checker.check(PREVIOUS, self.write(self.renewal(), 'signed.aklc', signed=True), now=NOW)
        self.assertTrue(signed[0].startswith('candidate: signed, sha256 '))

    def test_the_revision_must_be_exactly_the_next_one(self):
        for revision in (5, 4, 7, 100):
            self.rejected(self.renewal(revision=revision), f'revision is {revision}, expected 6')
        self.check(self.renewal(revision=9), revision=9)
        self.rejected(self.renewal(revision=5), 'expected 5', revision=5)  # never equal to the previous one

    def test_the_package_records_must_be_identical(self):
        changed = self.renewal()
        changed['packages'][0]['sha256'] = '0' * 64
        self.rejected(changed, f"changed or missing records: ['{changed['packages'][0]['package_id']}']")
        fewer = self.renewal()
        del fewer['packages'][-1]
        self.rejected(fewer, 'the package records differ')
        more = self.renewal()
        more['packages'].append(dict(more['packages'][0], package_id='extra-pack'))
        self.rejected(more, "'extra-pack'")
        reordered = self.renewal()
        reordered['packages'].reverse()
        self.rejected(reordered, 'the order differs')
        retagged = self.renewal()
        retagged['packages'][3]['tag'] = 'lang-r6-20261003'
        self.rejected(retagged, 'the package records differ')

    def test_the_window_must_be_exactly_the_requested_length(self):
        issued = NOW - 3600
        for days in (7, 20, 22, 31):
            self.rejected(self.renewal(expires_at=issued + days * 86400), f'the window is {days} days, expected exactly 21')
        self.check(self.renewal(expires_at=issued + 31 * 86400), days=31)
        self.check(self.renewal(expires_at=issued + 7 * 86400), days=7)
        self.rejected(self.renewal(expires_at=issued + 32 * 86400), '--days must be 1 to 31', days=32)
        self.rejected(self.renewal(), '--days must be 1 to 31', days=0)
        self.rejected(self.renewal(expires_at=issued + 21 * 86400 + 1), 'expected exactly 21')

    def test_the_window_must_have_started_and_not_long_ago(self):
        future = self.renewal(issued_at=NOW + 60, expires_at=NOW + 60 + 21 * 86400)
        self.rejected(future, 'has not started yet')
        stale = self.renewal(issued_at=NOW - 5 * 3600, expires_at=NOW - 5 * 3600 + 21 * 86400)
        self.rejected(stale, 'prepare the candidate again just before signing')
        self.check(stale, max_age_hours=6)

    def test_other_changes_are_refused(self):
        self.rejected(self.renewal(repository='someone/else'), 'repository differs')
        extra = self.renewal()
        extra['note'] = 'x'
        self.rejected(extra, 'unexpected catalog fields')
        for bad in (6.0, True, '6', None, -1):
            self.rejected(self.renewal(revision=bad), 'is not a whole number')
        self.rejected(self.renewal(packages=[]), 'no packages')

    def test_malformed_files_are_refused(self):
        def attempt(text, fragment):
            path = self.directory / 'bad.json'
            path.write_text(text, encoding='utf-8')
            with self.assertRaises(checker.Rejected) as caught:
                checker.check(PREVIOUS, path, now=NOW)
            self.assertIn(fragment, str(caught.exception))

        text = json.dumps(self.renewal(), separators=(',', ':'))
        attempt('not json', 'not a JSON document')
        attempt('[]', 'unexpected fields')
        attempt(json.dumps({'format': 2, 'catalog': text}), 'not a format 1 catalog envelope')
        attempt(json.dumps({'format': 1, 'catalog': {'revision': 6}}), 'not a format 1 catalog envelope')
        attempt(json.dumps({'format': 1, 'catalog': 'nope'}), 'the catalog text is not JSON')
        attempt(json.dumps({'format': 1, 'catalog': text, 'extra': 1}), 'unexpected fields')
        attempt('{"format": 1, "format": 1, "catalog": ' + json.dumps(text) + '}', 'a field is written twice')
        attempt('x' * (checker.MAX_BYTES + 1), 'larger than')
        doubled = text.replace('"revision":6', '"revision":6,"revision":7', 1)
        self.assertNotEqual(doubled, text)
        attempt(json.dumps({'format': 1, 'catalog': doubled}), 'a field is written twice')
        with self.assertRaises(OSError):
            checker.check(PREVIOUS, self.directory / 'missing.json', now=NOW)

    def test_main_prints_the_summary_and_the_exit_status(self):
        good = self.write(self.renewal())
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = checker.main([str(PREVIOUS), str(good), '--now', str(NOW)])
        self.assertEqual(code, 0, err.getvalue())
        self.assertIn('CANDIDATE_OK', out.getvalue())
        self.assertIn('window 21 days', out.getvalue())
        bad = self.write(self.renewal(revision=5), 'bad.json')
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = checker.main([str(PREVIOUS), str(bad), '--now', str(NOW)])
        self.assertEqual((code, out.getvalue()), (1, ''))
        self.assertIn('REJECTED: revision is 5', err.getvalue())
        with contextlib.redirect_stderr(io.StringIO()) as missing:
            self.assertEqual(checker.main([str(PREVIOUS), str(self.directory / 'none.json')]), 1)
        self.assertIn('REJECTED', missing.getvalue())


if __name__ == '__main__':
    unittest.main()
