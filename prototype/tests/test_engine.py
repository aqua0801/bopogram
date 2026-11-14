"""Decoder, session and protocol tests with a tiny dictionary and a fake language model.

They do not need the trained models, so they run anywhere.
"""

import math
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from bopogram.decoder import Decoder
from bopogram.lexicon import Lexicon
from bopogram.server import handle
from bopogram.session import Session
from bopogram.user import UserModel

DICT = """我\tㄨㄛˇ\ttest
在\tㄗㄞˋ\ttest
再\tㄗㄞˋ\ttest
家\tㄐㄧㄚ\ttest
加\tㄐㄧㄚ\ttest
看\tㄎㄢˋ\ttest
看看\tㄎㄢˋ ㄎㄢˋ\ttest
的\t˙ㄉㄜ\ttest
"""
UNIGRAM = {"我": 0.2, "在": 0.15, "再": 0.1, "家": 0.1, "加": 0.05, "看": 0.1, "看看": 0.05, "的": 0.2}
BIGRAM = {("我", "在"): 0.4, ("在", "家"): 0.5, ("我", "再"): 0.2, ("再", "看看"): 0.6, ("在", "看看"): 0.05}


class FakeLM:
    """Bigram model over the toy vocabulary; state = (previous word,)."""

    def begin(self):
        return ("<s>",)

    def score(self, state, token):
        p = BIGRAM.get((state[0], token), 0.1 * UNIGRAM.get(token, 1e-6))
        return math.log(p), (token,)

    def unigram(self, token):
        return math.log(UNIGRAM.get(token, 1e-6))

    @staticmethod
    def map_token(tok, in_vocab):
        return tok if in_vocab(tok) else "<x>"


def make_session(user=None):
    tmp = tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False, encoding="utf-8")
    tmp.write(DICT)
    tmp.close()
    lexicon, lm = Lexicon(tmp.name), FakeLM()
    decoder = Decoder(lexicon, lm, extra_score=user.bonus if user else None)
    return Session(SimpleNamespace(lexicon=lexicon, lm=lm, decoder=decoder, user=user))


def type_keys(session, keys):
    for k in keys:
        session.key(k)


class DecoderTest(unittest.TestCase):
    def test_context_picks_homophone(self):
        s = make_session()
        type_keys(s, "ji3y94ru8 ")           # ㄨㄛˇ ㄗㄞˋ ㄐㄧㄚ
        self.assertEqual(s.preedit(), "我在家")
        s.escape()
        type_keys(s, "ji3y94d04d04")         # ㄨㄛˇ ㄗㄞˋ ㄎㄢˋ ㄎㄢˋ
        self.assertEqual(s.preedit(), "我再看看")

    def test_earlier_choice_is_revised_while_composing(self):
        s = make_session()
        type_keys(s, "ji3y94")
        self.assertEqual(s.preedit(), "我在")
        type_keys(s, "d04d04")
        self.assertEqual(s.preedit(), "我再看看")

    def test_unknown_syllable_does_not_break_decoding(self):
        s = make_session()
        type_keys(s, "ji3x8 y94")            # ㄌㄚ is not in the toy dictionary
        self.assertTrue(s.preedit().startswith("我"))
        self.assertTrue(s.preedit().endswith(("在", "再")))


class SessionTest(unittest.TestCase):
    def test_pending_symbols_and_backspace(self):
        s = make_session()
        type_keys(s, "ji")
        self.assertEqual((s.pending, s.preedit()), ("ㄨㄛ", "ㄨㄛ"))
        s.backspace()
        self.assertEqual(s.pending, "ㄨ")
        type_keys(s, "i3y94")
        s.backspace()
        self.assertEqual(s.preedit(), "我")

    def test_select_pins_a_candidate(self):
        s = make_session()
        type_keys(s, "ji3y94ru8 ")
        s.move(-2)
        words = [c["word"] for c in s.candidates()]
        self.assertIn("再", words)
        self.assertTrue(s.select(words.index("再")))
        self.assertEqual(s.preedit()[:2], "我再")
        type_keys(s, "d04")                  # later typing must not undo the choice
        self.assertEqual(s.preedit()[:2], "我再")

    def test_commit_moves_text_to_context_and_never_changes_it(self):
        s = make_session()
        type_keys(s, "ji3y94")
        self.assertEqual(s.commit(), "我在")
        self.assertEqual(s.context, "我在")
        type_keys(s, "ru8 ")
        self.assertEqual(s.preedit(), "家")    # scored after the committed 在
        self.assertEqual(s.context, "我在")

    def test_learning_can_be_switched_off(self):
        with tempfile.TemporaryDirectory() as d:
            user = UserModel(Path(d) / "user.json")
            s = make_session(user)
            s.learn = False
            type_keys(s, "ji3")
            s.commit()
            self.assertEqual(user.unigrams, {})
            s.learn = True
            type_keys(s, "ji3y94")
            s.commit()
            self.assertEqual(user.unigrams.get("我"), 1)
            self.assertTrue((Path(d) / "user.json").exists())

    def test_user_model_changes_ranking(self):
        user = UserModel()
        for _ in range(5):
            user.learn(["我", "再", "加"], corrected=True)
        s = make_session(user)
        type_keys(s, "ji3y94ru8 ")
        self.assertEqual(s.preedit(), "我再加")


class ProtocolTest(unittest.TestCase):
    def test_requests(self):
        s = make_session()
        for k in "ji3y94":
            self.assertTrue(handle(s, {"op": "key", "key": k})["ok"])
        self.assertFalse(handle(s, {"op": "key", "key": "!"})["ok"])
        res = handle(s, {"op": "candidates"})
        self.assertTrue(res["candidates"])
        self.assertEqual(handle(s, {"op": "commit"})["commit"], "我在")
        self.assertFalse(handle(s, {"op": "nonsense"})["ok"])
        handle(s, {"op": "learning", "enabled": False})
        self.assertFalse(s.learn)


if __name__ == "__main__":
    unittest.main()
