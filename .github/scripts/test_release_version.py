import unittest
from release_version import next_version, parse


class VersionTests(unittest.TestCase):
    def test_patch(self):
        self.assertEqual(next_version(['0.1.15']), '0.1.16')

    def test_minor_rollover(self):
        self.assertEqual(next_version(['0.1.98']), '0.2.0')

    def test_major_rollover(self):
        self.assertEqual(next_version(['0.98.98']), '1.0.0')

    def test_highest_published_version(self):
        self.assertEqual(next_version(['0.1.12', 'v0.1.15', 'v0.1.9']), '0.1.16')

    def test_invalid(self):
        with self.assertRaises(ValueError):
            parse('v1.2.3-beta')


if __name__ == '__main__':
    unittest.main()
