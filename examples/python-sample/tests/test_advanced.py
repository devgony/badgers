import unittest

from pkg.features.math.advanced import square


class AdvancedMathTest(unittest.TestCase):
    def test_square(self):
        self.assertEqual(square(4), 16)


if __name__ == "__main__":
    _ = unittest.main()
