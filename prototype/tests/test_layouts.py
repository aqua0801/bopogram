import unittest

from bopogram.layouts import DACHEN, LayoutError


class DachenTest(unittest.TestCase):
    def test_basic(self):
        # 我在家: ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ
        self.assertEqual(DACHEN.to_syllables("ji3y94ru8 "), ["ㄨㄛˇ", "ㄗㄞˋ", "ㄐㄧㄚ"])

    def test_neutral_tone_is_prefixed(self):
        self.assertEqual(DACHEN.to_syllables("2k7"), ["˙ㄉㄜ"])

    def test_round_trip(self):
        syls = ["ㄓㄨㄤ", "ㄒㄩㄝˊ", "˙ㄇㄜ", "ㄦˊ", "ㄩˋ"]
        self.assertEqual(DACHEN.to_syllables(DACHEN.to_keys(syls)), syls)

    def test_missing_tone(self):
        with self.assertRaises(LayoutError):
            DACHEN.to_syllables("ji")

    def test_unknown_key(self):
        with self.assertRaises(LayoutError):
            DACHEN.to_syllables("ji3!")


if __name__ == "__main__":
    unittest.main()
