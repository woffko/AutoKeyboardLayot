import unittest
from fetch_dependency_notices import PublicGitHubRedirects, safe_path


class NoticeDownloadTests(unittest.TestCase):
    def test_paths_remain_relative(self):
        self.assertEqual(str(safe_path('LICENSES/MIT.txt')), 'LICENSES/MIT.txt')
        for path in ('../LICENSE', '/LICENSE', 'a/../LICENSE', 'a//LICENSE', 'a\\LICENSE', '', 'bad\0name'):
            with self.subTest(path=path), self.assertRaises(ValueError):
                safe_path(path)

    def test_redirects_cannot_leave_public_github(self):
        handler = PublicGitHubRedirects()
        for url in ('http://api.github.com/a', 'https://example.com/LICENSE', 'file:///etc/passwd',
                    'https://name@raw.githubusercontent.com/a'):
            with self.subTest(url=url), self.assertRaises(ValueError):
                handler.redirect_request(None, None, 302, '', {}, url)


if __name__ == '__main__':
    unittest.main()
