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
一\tㄧ\ttest
一\tㄧˋ\ttest:var
意\tㄧˋ\ttest
ㄨ\tㄨ\tsymbol
地\t˙ㄉㄜ\ttest
"""
UNIGRAM = {"一": 0.3, "意": 0.01, "地": 0.01, "我": 0.2, "在": 0.15, "再": 0.1, "家": 0.1, "加": 0.05, "看": 0.1, "看看": 0.05, "的": 0.2}
BIGRAM = {("看看", "地"): 0.9, ("我", "在"): 0.4, ("在", "家"): 0.5, ("我", "再"): 0.2, ("再", "看看"): 0.6, ("在", "看看"): 0.05}


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
    def new_decoder():
        return Decoder(lexicon, lm, extra_score=user.bonus if user else None)

    return Session(SimpleNamespace(lexicon=lexicon, lm=lm, new_decoder=new_decoder, user=user))


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


    def test_tone_sandhi_needs_the_right_following_tone(self):
        s = make_session()
        type_keys(s, "u4")                   # ㄧˋ alone: not 一, although 一 is far more frequent
        self.assertEqual(s.preedit(), "意")
        type_keys(s, "ru8 ")                 # followed by a first tone (ㄐㄧㄚ): the sandhi reading applies
        self.assertEqual(s.preedit(), "一家")
        s.escape()
        type_keys(s, "u4y94")                # followed by a fourth tone: ㄧˋ cannot be 一
        self.assertEqual(s.preedit()[0], "意")

    def test_bopomofo_symbol_is_a_candidate(self):
        s = make_session()
        type_keys(s, "j ")                   # ㄨ + first tone: nothing else is read ㄨ in the toy dictionary
        self.assertIn("ㄨ", [c["word"] for c in s.candidates()])

    def test_position_scores_shift_the_choice_but_never_remove_a_word(self):
        s = make_session()
        type_keys(s, "ji3y94ru8 ")
        self.assertEqual(s.preedit(), "我在家")
        d = s.decoder
        d.rescore([{}, {"再": -0.1, "在": -9.0}, {}], weight=1.0)
        self.assertEqual(d.best_text(), "我再家")
        # A character the model gives no chance at all is only penalised down to the floor,
        # so a pinned or strongly supported word still wins.
        d.rescore([{}, {"再": 0.0}, {}], weight=0.05, floor=-12.0)
        self.assertEqual(d.best_text(), "我在家")
        d.rescore(None)
        self.assertEqual(d.best_text(), "我在家")


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
        s.move(-2)                           # cursor before 在: the list is for 在
        words = [c["word"] for c in s.candidates()]
        self.assertIn("再", words)
        self.assertTrue(s.select(words.index("再")))
        self.assertEqual(s.preedit()[:2], "我再")
        type_keys(s, "d04")                  # later typing must not undo the choice
        self.assertEqual(s.preedit()[:2], "我再")

    def test_editing_in_the_middle(self):
        s = make_session()
        type_keys(s, "ji3ru8 ")              # 我家
        s.move(-1)
        type_keys(s, "y94")                  # insert ㄗㄞˋ between them
        self.assertEqual((s.preedit(), s.cursor), ("我在家", 2))
        s.backspace()                        # removes the item before the cursor
        self.assertEqual((s.preedit(), s.cursor), ("我家", 1))
        s.delete()                           # removes the item after the cursor
        self.assertEqual(s.preedit(), "我")
        type_keys(s, "y94d04d04")
        self.assertEqual(s.preedit(), "我再看看")
        s.move_to(False)
        self.assertEqual(s.cursor, 0)
        s.delete()
        self.assertEqual(s.preedit()[1:], "看看")
        s.move_to(True)
        self.assertEqual(s.cursor, 3)

    def test_pending_syllable_shows_at_the_cursor(self):
        s = make_session()
        type_keys(s, "ji3ru8 ")
        s.move(-1)
        type_keys(s, "y9")
        self.assertEqual((s.preedit(), s.state()["caret"]), ("我ㄗㄞ家", 3))
        s.move(-1)                           # ignored while a syllable is in progress
        self.assertEqual(s.cursor, 1)
        s.escape()                           # first Escape drops the syllable only
        self.assertEqual(s.preedit(), "我家")
        s.escape()
        self.assertEqual(s.preedit(), "")

    def test_pins_follow_edits(self):
        s = make_session()
        type_keys(s, "ji3y94ru8 ")
        s.move(-2)
        words = [c["word"] for c in s.candidates()]
        s.select(words.index("再"))
        s.move_to(False)
        type_keys(s, "d04")                  # insert before everything: the pin moves along
        self.assertEqual(s.preedit()[1:3], "我再")
        s.move_to(False)
        s.move(3)
        s.backspace()                        # deleting the pinned word removes the pin
        self.assertEqual(s.decoder.pinned, {})

    def test_space_symbols_and_text(self):
        s = make_session()
        self.assertFalse(s.key(" "))         # nothing composed: the application gets the space
        type_keys(s, "ji3")
        self.assertTrue(s.key(" "))          # a space inside the buffer
        self.assertTrue(s.symbol("<"))
        self.assertTrue(s.text("OK"))
        self.assertFalse(s.symbol("a"))
        type_keys(s, "y94ru8 ")
        self.assertEqual(s.preedit(), "我 ，OK在家")
        s.move_to(False)
        s.move(2)
        self.assertEqual(s.candidates(), []) # a literal has no candidates
        self.assertEqual(s.commit(), "我 ，OK在家")

    def test_cycle_changes_the_word_without_a_list(self):
        s = make_session()
        type_keys(s, "ji3y94")
        self.assertEqual(s.preedit(), "我在")
        self.assertTrue(s.cycle())
        self.assertEqual(s.preedit(), "我再")
        self.assertTrue(s.cycle())
        self.assertEqual(s.preedit(), "我在")
        s.move(-2)
        self.assertFalse(s.cycle())          # 我 has no other candidate

    def test_long_buffer_commits_its_first_words(self):
        s = make_session()
        for _ in range(12):
            type_keys(s, "ji3y94ru8 ")
        self.assertLessEqual(s.state()["syllables"], 30)
        flushed = s.take_flushed()
        self.assertTrue(flushed.startswith("我在家"))
        self.assertTrue(s.context.endswith(flushed))
        self.assertEqual(len(flushed) + s.state()["syllables"], 36)
        self.assertEqual(s.cursor, s.state()["syllables"])

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

    def test_any_response_may_carry_commit(self):
        s = make_session()
        seen = ""
        for _ in range(12):
            for k in "ji3y94ru8 ":
                seen += handle(s, {"op": "key", "key": k}).get("commit", "")
        self.assertTrue(seen.startswith("我在家"))
        res = handle(s, {"op": "commit"})
        self.assertEqual(len(seen + res["commit"]), 36)
        for op in ("home", "end", "delete", "cycle"):
            self.assertIn("ok", handle(s, {"op": op}))

    def test_style_switches_particle_choice(self):
        # The toy model prefers 地 after 看看; casual style still writes 的.
        s = make_session()
        s.context = "看看"
        s.escape()
        self.assertTrue(handle(s, {"op": "style", "style": "formal"})["ok"])
        type_keys(s, "2k7")
        self.assertEqual(s.preedit(), "地")
        res = handle(s, {"op": "style", "style": "casual"})
        self.assertEqual((res["preedit"], res["style"]), ("的", "casual"))
        # A candidate picked by hand wins over the style.
        words = [c["word"] for c in s.candidates()]
        s.select(words.index("地"))
        self.assertEqual(s.preedit(), "地")
        self.assertFalse(handle(s, {"op": "style", "style": "nonsense"})["ok"])


if __name__ == "__main__":
    unittest.main()
