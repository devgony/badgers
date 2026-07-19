import unittest

from pkg.calc import add, classify, fizzbuzz


class CalcTest(unittest.TestCase):
    def test_add(self):
        self.assertEqual(add(1, 2), 3)

    def test_classify_positive(self):
        self.assertEqual(classify(5), "positive")

    def test_classify_negative(self):
        self.assertEqual(classify(-3), "negative")

    def test_fizzbuzz(self):
        self.assertEqual(fizzbuzz(3), "fizz")
        self.assertEqual(fizzbuzz(5), "buzz")
        self.assertEqual(fizzbuzz(15), "fizzbuzz")
        self.assertEqual(fizzbuzz(7), "7")


if __name__ == "__main__":
    unittest.main()
