"""The user's dictionary: added words, excluded text and the protocol operations.

Like test_engine.py these use a tiny dictionary and a fake language model.
"""

import math
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from bopogram import dictops
from bopogram.decoder import Decoder
from bopogram.layouts.dachen import DACHEN
from bopogram.lexicon import Lexicon
from bopogram.server import handle
from bopogram.session import Session
from bopogram.user import UserModel
from bopogram.userdict import UserDict, valid_syllable

DICT = """視\tㄕˋ\ttest
是\tㄕˋ\ttest
頻\tㄆㄧㄣˊ\ttest
貧\tㄆㄧㄣˊ\ttest
視頻\tㄕˋ ㄆㄧㄣˊ\ttest
道\tㄉㄠˋ\ttest
到\tㄉㄠˋ\ttest
波\tㄅㄛ\ttest
長\tㄓㄤˇ\ttest
長\tㄔㄤˊ\ttest
常\tㄔㄤˊ\ttest
大\tㄉㄚˋ\ttest
長大\tㄓㄤˇ ㄉㄚˋ\ttest
我\tㄨㄛˇ\ttest
"""
UNIGRAM = {"視": 0.2, "是": 0.01, "頻": 0.2, "貧": 0.01, "視頻": 0.3, "道": 0.1, "到": 0.1,
           "波": 0.1, "長": 0.1, "常": 0.2, "大": 0.2, "長大": 0.1, "我": 0.2}
BIGRAM = {("視", "頻"): 0.9, ("頻", "道"): 0.9, ("視頻", "道"): 0.1}
BONUS = 20.0


class FakeLM:
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


class Env:
    """An engine over the toy dictionary with a user dictionary in a temp dir."""

    def __init__(self, user_files=("userdict.tsv", "excluded.tsv"), user_model=True):
        self.dir = tempfile.TemporaryDirectory()
        d = Path(self.dir.name)
        (d / "dict.tsv").write_text(DICT, encoding="utf-8")
        lexicon, lm = Lexicon(d / "dict.tsv"), FakeLM()
        files = [d / f if f else None for f in user_files]
        self.userdict = UserDict(*files)
        lexicon.attach(self.userdict, BONUS)
        user = UserModel(d / "user.json") if user_model else None
        self.engine = SimpleNamespace(
            lexicon=lexicon, lm=lm, user=user, userdict=self.userdict,
            new_decoder=lambda: Decoder(lexicon, lm, extra_score=user.bonus if user else None))
        self.path = d

    def session(self):
        return Session(self.engine)

    def close(self):
        self.dir.cleanup()


def type_syls(s, reading):
    for k in DACHEN.to_keys(reading.split()):
        s.key(k)


def op(s, **req):
    return handle(s, req)


class EnvCase(unittest.TestCase):
    def setUp(self):
        self.env = Env()
        self.s = self.env.session()

    def tearDown(self):
        self.env.close()


class ParseTest(unittest.TestCase):
    def test_files_with_comments_bad_lines_and_bom(self):
        with tempfile.TemporaryDirectory() as d:
            u, e = Path(d) / "u.tsv", Path(d) / "e.tsv"
            u.write_text("﻿# my words\n\n波波\tㄅㄛ ㄅㄛ\tuser\n壞\n短\tㄅㄛ ㄅㄛ\tuser\n怪\tㄅㄅ\tuser\n"
                         "波波\tㄅㄛ ㄅㄛ\tuser\n", encoding="utf-8")
            e.write_text("視頻\n是\tㄕˋ\n\n# x\n太長了太長了太長了\n", encoding="utf-8")
            with mock.patch("sys.stderr") as err:
                ud = UserDict(u, e)
            self.assertEqual(ud.user, {"波波": [("ㄅㄛ", "ㄅㄛ")]})
            self.assertEqual(ud.excluded, {"視頻": [None], "是": [("ㄕˋ",)]})
            logged = "".join(str(c.args[0]) for c in err.write.call_args_list)
            self.assertIn("line 4", logged)
            self.assertNotIn("壞", logged)  # line numbers only, never the content

    def test_missing_files_are_empty_and_created_on_first_write(self):
        with tempfile.TemporaryDirectory() as d:
            ud = UserDict(Path(d) / "sub" / "u.tsv", Path(d) / "sub" / "e.tsv")
            self.assertEqual(ud.entries(), [])
            ud.add("波波", ("ㄅㄛ", "ㄅㄛ"))
            self.assertEqual((Path(d) / "sub" / "u.tsv").read_text(encoding="utf-8"), "波波\tㄅㄛ ㄅㄛ\tuser\n")
            self.assertFalse((Path(d) / "sub" / "e.tsv").exists())

    def test_syllable_check(self):
        for ok in ("ㄅㄛ", "ㄓ", "ㄧㄤˊ", "˙ㄇㄜ", "ㄦˋ", "ㄨ"):
            self.assertTrue(valid_syllable(ok), ok)
        for bad in ("", "ˊ", "ㄅ", "ㄅㄅ", "ㄛㄅ", "˙ㄇㄜˊ", "a", "ㄅㄛˊˊ"):
            self.assertFalse(valid_syllable(bad), bad)


class AddTest(EnvCase):
    def test_added_word_is_first_for_its_reading_in_every_session(self):
        type_syls(self.s, "ㄅㄛ ㄅㄛ")
        self.assertEqual(self.s.preedit(), "波波")
        res = op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        self.assertTrue(res["ok"])
        self.assertEqual(res["preedit"], "波波")      # not decoded again until the next key
        self.s.escape()
        type_syls(self.s, "ㄅㄛ ㄅㄛ")
        self.assertEqual(self.s.preedit(), "伯伯")
        other = self.env.session()
        type_syls(other, "ㄅㄛ ㄅㄛ")
        self.assertEqual(other.preedit(), "伯伯")
        self.assertEqual(other.candidates()[0]["word"], "伯伯")
        text = (self.env.path / "userdict.tsv").read_text(encoding="utf-8")
        self.assertEqual(text, "伯伯\tㄅㄛ ㄅㄛ\tuser\n")

    def test_reranker_keeps_user_words(self):
        from bopogram.engine import Engine

        class FakeCharLM:  # prefers anything but the user's word
            def score(self, context, texts):
                return [0.0 if "伯伯" in t else 50.0 for t in texts]

        eng = self.env.engine
        eng.neural, eng.charlm, eng.charlm_top, eng.charlm_weight = None, FakeCharLM(), 5, 1.0
        eng.rescore = lambda decoder, context: Engine.rescore(eng, decoder, context)
        op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        self.s.decoder.merge = False
        type_syls(self.s, "ㄅㄛ ㄅㄛ ㄨㄛˇ")
        self.assertEqual(self.s.preedit(), "伯伯我")

    def test_change_shows_on_the_next_key_for_the_whole_buffer(self):
        type_syls(self.s, "ㄅㄛ ㄅㄛ ㄨㄛˇ")
        op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        self.assertEqual(self.s.preedit(), "波波我")
        type_syls(self.s, "ㄨㄛˇ")
        self.assertEqual(self.s.preedit(), "伯伯我我")

    def test_commit_uses_the_new_dictionary(self):
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ")
        op(self.s, op="dict_exclude", text="視頻")
        self.assertNotEqual(op(self.s, op="commit")["commit"], "視頻")

    def test_span_form_takes_the_typed_reading(self):
        type_syls(self.s, "ㄔㄤˊ ㄉㄚˋ")
        self.s.move_to(False)
        cands = self.s.candidates()
        self.s.select(next(i for i, c in enumerate(cands) if c["word"] == "長"))
        self.assertEqual(self.s.preedit(), "長大")
        self.assertTrue(op(self.s, op="dict_add", start=0, length=2)["ok"])
        self.assertEqual(self.env.userdict.user, {"長大": [("ㄔㄤˊ", "ㄉㄚˋ")]})

    def test_span_with_a_symbol_or_unconverted_syllable_is_refused(self):
        type_syls(self.s, "ㄅㄛ")
        self.s.text("!")
        res = op(self.s, op="dict_add", start=0, length=2)
        self.assertEqual((res["ok"], res["reason"]), (False, "bad_reading"))
        res = op(self.s, op="dict_add", start=1, length=5)
        self.assertEqual(res["reason"], "not_found")

    def test_adding_lifts_an_exclusion(self):
        op(self.s, op="dict_exclude", text="波波")
        op(self.s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")
        self.assertEqual(self.env.userdict.excluded, {})

    def test_user_words_count_for_learning(self):
        op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        type_syls(self.s, "ㄅㄛ ㄅㄛ")
        self.s.commit()
        self.assertEqual(self.env.engine.user.unigrams.get("伯伯"), 1)


class RemoveTest(EnvCase):
    def test_remove_dictionary_word_then_restore(self):
        res = op(self.s, op="dict_remove", word="視頻")
        self.assertEqual((res["ok"], res["removed"]), (True, "excluded"))
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ")
        self.assertNotIn("視頻", [c["word"] for c in self.s.candidates()])
        self.assertTrue(op(self.s, op="dict_restore", word="視頻")["ok"])
        self.s.escape()
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ")
        self.assertEqual(self.s.preedit(), "視頻")

    def test_one_reading_only(self):
        op(self.s, op="dict_remove", word="長", reading="ㄔㄤˊ")
        self.assertNotIn("長", self.env.engine.lexicon.candidates(("ㄔㄤˊ",)))
        self.assertIn("長", self.env.engine.lexicon.candidates(("ㄓㄤˇ",)))

    def test_remove_user_word_and_word_in_both(self):
        op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        self.assertEqual(op(self.s, op="dict_remove", word="伯伯")["removed"], "user")
        self.assertEqual(self.env.userdict.entries(), [])
        op(self.s, op="dict_add", word="長大", reading="ㄓㄤˇ ㄉㄚˋ")
        self.assertEqual(op(self.s, op="dict_remove", word="長大")["removed"], "excluded")
        self.assertNotIn("長大", self.env.engine.lexicon.candidates(("ㄓㄤˇ", "ㄉㄚˋ")))

    def test_remove_a_candidate_by_span(self):
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ")
        res = op(self.s, op="dict_remove", start=0, length=2, word="長大")
        self.assertEqual(res["reason"], "not_found")
        res = op(self.s, op="dict_remove", start=0, length=1, word="是")
        self.assertEqual(res["removed"], "excluded")
        self.assertEqual(self.env.userdict.excluded, {"是": [("ㄕˋ",)]})

    def test_forgets_learned_words_and_pairs(self):
        user = self.env.engine.user
        user.learn(["視頻", "道"])
        user.learn(["視", "頻"])
        user.learn(["我", "到"])
        op(self.s, op="dict_exclude", text="視頻")
        self.assertEqual(user.unigrams, {"道": 1, "視": 1, "頻": 1, "我": 1, "到": 1})
        self.assertEqual(user.bigrams, {"我\t到": 1})
        self.assertNotIn("視頻", (self.env.path / "user.json").read_text(encoding="utf-8"))

    def test_last_candidate(self):
        res = op(self.s, op="dict_exclude", text="我")
        self.assertEqual((res["ok"], res["reason"]), (False, "last_candidate"))
        res = op(self.s, op="dict_remove", word="波")
        self.assertEqual(res["reason"], "last_candidate")
        self.assertEqual(op(self.s, op="dict_exclude", text="長")["reason"], "last_candidate")  # ㄓㄤˇ
        self.assertTrue(op(self.s, op="dict_exclude", text="長", reading="ㄔㄤˊ")["ok"])  # 常 is left
        self.assertEqual(op(self.s, op="dict_exclude", text="常")["reason"], "last_candidate")


class ExclusionTest(EnvCase):
    def test_excluded_text_never_forms_across_words(self):
        op(self.s, op="dict_exclude", text="視頻")
        for context in ("", "我", "視", "頻"):
            for reading in ("ㄕˋ ㄆㄧㄣˊ", "ㄕˋ ㄆㄧㄣˊ ㄉㄠˋ", "ㄨㄛˇ ㄕˋ ㄆㄧㄣˊ ㄕˋ ㄆㄧㄣˊ"):
                self.s.set_context(context)
                type_syls(self.s, reading)
                self.assertNotIn("視頻", self.s.preedit(), (context, reading))
                self.assertNotIn("視頻", self.s.commit())

    def test_longer_excluded_text_across_two_words(self):
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ ㄉㄠˋ")
        self.assertEqual(self.s.preedit(), "視頻道")
        op(self.s, op="dict_exclude", text="頻道")
        op(self.s, op="dict_exclude", text="視頻道")
        self.s.escape()
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ ㄉㄠˋ")
        self.assertNotIn("頻道", self.s.preedit())

    def test_reading_specific_exclusion(self):
        op(self.s, op="dict_exclude", text="長大", reading="ㄔㄤˊ ㄉㄚˋ")
        type_syls(self.s, "ㄓㄤˇ ㄉㄚˋ")
        self.assertEqual(self.s.preedit(), "長大")

    def test_hand_picked_text_stays(self):
        op(self.s, op="dict_exclude", text="視頻")
        type_syls(self.s, "ㄕˋ ㄆㄧㄣˊ")
        self.s.move_to(False)
        cands = self.s.candidates()
        self.assertNotIn("視頻", [c["word"] for c in cands])
        self.s.select(next(i for i, c in enumerate(cands) if c["word"] == "視"))
        cands = self.s.candidates()
        self.s.select(next(i for i, c in enumerate(cands) if c["word"] == "頻"))
        self.assertEqual(self.s.preedit(), "視頻")
        type_syls(self.s, "ㄉㄠˋ")
        self.assertTrue(self.s.preedit().startswith("視頻"))
        self.assertTrue(self.s.commit().startswith("視頻"))
        self.assertNotIn("視頻", self.env.engine.user.unigrams)  # not learned from

    def test_every_path_blocked_shows_the_syllable(self):
        op(self.s, op="dict_exclude", text="波波")
        type_syls(self.s, "ㄅㄛ ㄅㄛ")
        self.assertNotIn("波波", self.s.preedit())

    def test_empty_dictionary_changes_nothing(self):
        plain = Lexicon(self.env.path / "dict.tsv")
        for reading in ("ㄕˋ ㄆㄧㄣˊ ㄉㄠˋ", "ㄓㄤˇ ㄉㄚˋ ㄨㄛˇ"):
            a = Decoder(plain, FakeLM()).decode(reading.split(), n=5)
            b = Decoder(self.env.engine.lexicon, FakeLM()).decode(reading.split(), n=5)
            self.assertEqual(a, b)


class ProtocolTest(EnvCase):
    def test_errors(self):
        cases = [(dict(op="dict_add", word="波波", reading="ㄅㄛ ㄅ"), "bad_reading"),
                 (dict(op="dict_add", word="波波", reading="ㄅㄛ"), "length_mismatch"),
                 (dict(op="dict_add", word="波" * 9, reading=" ".join(["ㄅㄛ"] * 9)), "too_long"),
                 (dict(op="dict_remove", word="沒有"), "not_found"),
                 (dict(op="dict_restore", word="沒有"), "not_found")]
        for req, reason in cases:
            res = op(self.s, **req)
            self.assertEqual((res["ok"], res["reason"]), (False, reason), req)
            self.assertIn("preedit", res)
        op(self.s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")
        self.assertEqual(op(self.s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")["reason"], "duplicate")

    def test_disabled(self):
        env = Env(user_files=(None, "excluded.tsv"))
        try:
            s = env.session()
            self.assertEqual(op(s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")["reason"], "disabled")
            self.assertTrue(op(s, op="dict_exclude", text="視頻")["ok"])
        finally:
            env.close()
        s = Session(SimpleNamespace(**{**vars(self.env.engine), "userdict": None}))
        self.assertEqual(op(s, op="dict_list")["reason"], "disabled")

    def test_list_and_reload(self):
        op(self.s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")
        op(self.s, op="dict_exclude", text="視頻")
        self.assertEqual(op(self.s, op="dict_list")["entries"],
                         [{"word": "波波", "reading": "ㄅㄛ ㄅㄛ", "kind": "user"},
                          {"word": "視頻", "reading": "", "kind": "excluded"}])
        (self.env.path / "excluded.tsv").write_text("", encoding="utf-8")
        self.assertTrue(op(self.s, op="dict_reload")["ok"])
        self.assertIn("視頻", self.env.engine.lexicon.candidates(("ㄕˋ", "ㄆㄧㄣˊ")))

    def test_failed_write_keeps_the_old_file(self):
        op(self.s, op="dict_add", word="波波", reading="ㄅㄛ ㄅㄛ")
        path = self.env.path / "userdict.tsv"
        before = path.read_text(encoding="utf-8")
        with mock.patch("bopogram.userdict.os.replace", side_effect=OSError("disk full")):
            res = op(self.s, op="dict_add", word="伯伯", reading="ㄅㄛ ㄅㄛ")
        self.assertEqual((res["ok"], res["reason"]), (False, "write_failed"))
        self.assertEqual(path.read_text(encoding="utf-8"), before)
        self.assertEqual(self.env.userdict.user, {"波波": [("ㄅㄛ", "ㄅㄛ")]})  # nor the memory

    def test_readings(self):
        res = op(self.s, op="readings", text="長大")
        self.assertEqual(res["readings"], [{"reading": "ㄓㄤˇ ㄉㄚˋ", "known": True}])
        res = op(self.s, op="readings", text="長長")
        self.assertEqual(len(res["readings"]), 4)
        self.assertFalse(any(r["known"] for r in res["readings"]))
        self.assertEqual(op(self.s, op="readings", text="長" * 8)["readings"].__len__(), 16)
        self.assertEqual(op(self.s, op="readings", text="沒")["readings"], [])
        op(self.s, op="dict_exclude", text="長大")
        self.assertFalse(op(self.s, op="readings", text="長大")["readings"][0]["known"])


if __name__ == "__main__":
    unittest.main()
