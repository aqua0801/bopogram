"""Typing log and review store. Run from prototype/: python -m unittest tests.test_typing_log"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

from bopogram.typing_log import TypingLog
from tests.test_engine import handle, make_session, type_keys

_spec = importlib.util.spec_from_file_location(
    "review_log", Path(__file__).resolve().parent.parent / "tools" / "review_log.py")
review_log = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(review_log)


def read(path: Path) -> list[dict]:
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]


class TypingLogTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.path = Path(self.dir.name) / "typing_log.jsonl"
        self.s = make_session()
        self.s.engine.typing_log = TypingLog(self.path)

    def tearDown(self):
        self.dir.cleanup()

    def test_commit_records_first_choice_and_correction(self):
        s = self.s
        type_keys(s, "ji3y94")
        s.cycle()                                    # 在 -> 再, picked by hand
        s.commit()
        (rec,) = read(self.path)
        self.assertEqual((rec["first"], rec["text"]), ("我在", "我再"))
        self.assertEqual(rec["items"], ["ㄨㄛˇ", "ㄗㄞˋ"])
        self.assertEqual(rec["picked"], [[1, 1, "再"]])
        self.assertFalse(rec["auto"])
        type_keys(s, "ru8 ")
        s.commit()
        self.assertEqual(read(self.path)[1]["context"], "我再")

    def test_nothing_is_recorded_while_learning_is_off(self):
        s = self.s
        handle(s, {"op": "learning", "enabled": False})
        type_keys(s, "ji3y94")
        handle(s, {"op": "commit"})
        self.assertFalse(self.path.exists())
        handle(s, {"op": "learning", "enabled": True})
        type_keys(s, "ji3")
        handle(s, {"op": "commit"})
        self.assertEqual(len(read(self.path)), 1)

    def test_letters_and_digits_are_never_stored(self):
        s = self.s
        s.context = "帳號abc123"
        s.escape()
        type_keys(s, "ji3")
        s.text("pw9")
        s.symbol("<")
        s.commit()
        raw = self.path.read_text(encoding="utf-8")
        for secret in ("abc", "123", "pw9"):
            self.assertNotIn(secret, raw)
        (rec,) = read(self.path)
        self.assertEqual(rec["text"], "我＊＊＊，")
        self.assertEqual(rec["context"], "帳號＊＊＊＊＊＊")
        s.text("abc") if s.state()["syllables"] else None
        self.assertEqual(len(read(self.path)), 1)

    def test_text_without_bopomofo_is_not_recorded(self):
        log = TypingLog(self.path)
        self.assertFalse(log.write(["=，", "= "], "， ", "， ", [], "", "casual", False))
        self.assertFalse(self.path.exists())

    def test_automatic_commit_is_recorded_and_size_is_capped(self):
        s = self.s
        for _ in range(12):
            type_keys(s, "ji3y94ru8 ")
        recs = read(self.path)
        self.assertTrue(recs and all(r["auto"] for r in recs))
        self.assertEqual("".join(r["text"] for r in recs) + s.preedit(), "我在家" * 12)
        small = TypingLog(self.path, max_bytes=10)
        self.assertFalse(small.write(["ㄨㄛˇ"], "我", "我", [], "", "casual", False))


class ReviewStoreTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.userdata = Path(self.dir.name)
        log = TypingLog(self.userdata / review_log.LOG_NAME)
        log.write(["ㄨㄛˇ", "ㄗㄞˋ"], "我在", "我再", [(1, 1, "再")], "上文", "casual", False)
        log.write(["ㄨㄛˇ", "ㄗㄞˋ"], "我在", "我再", [(1, 1, "再")], "上文", "casual", False)
        log.write(["ㄐㄧㄚ"], "家", "家", [], "王小明的", "casual", False)
        log.write(["ㄎㄢˋ"], "看", "看", [], "", "casual", False)
        self.store = review_log.Store(self.userdata)

    def tearDown(self):
        self.store.db.close()
        self.dir.cleanup()

    def test_import_is_incremental_and_merges_duplicates(self):
        st = self.store
        self.assertEqual(st.import_new(), 4)
        self.assertEqual(st.import_new(), 0)
        self.assertEqual(st.stats()[review_log.PENDING], 3)
        changed = st.ids(review_log.FILTERS[0][1])
        self.assertEqual(len(changed), 1)
        self.assertEqual(st.get(changed[0])["count"], 2)

    def test_only_approved_records_are_exported(self):
        st = self.store
        st.import_new()
        a, b, c = st.ids("1 = 1")
        st.decide(a, review_log.APPROVED)
        st.decide(b, review_log.APPROVED_NO_CONTEXT)   # the context holds a name
        out = self.userdata / "export.jsonl"
        self.assertEqual(st.export(out), 2)
        text = out.read_text(encoding="utf-8")
        self.assertNotIn("王小明", text)
        self.assertNotIn("看", text)                    # still pending: not exported
        self.assertNotIn('"t"', text)                   # no timestamps
        rows = read(out)
        self.assertEqual((rows[0]["context"], rows[0]["count"], rows[1]["context"]), ("上文", 2, ""))

    def test_reject_by_text_and_clean_up(self):
        st = self.store
        st.import_new()
        for rec_id in st.ids("1 = 1"):
            st.decide(rec_id, review_log.APPROVED)
        self.assertEqual(st.reject_containing("王小明"), 1)
        self.assertEqual(st.reject_containing(""), 0)
        st.clear_raw_log()
        self.assertEqual((self.userdata / review_log.LOG_NAME).read_text(encoding="utf-8"), "")
        self.assertEqual(st.delete_rejected(), 1)
        self.assertEqual(st.stats()[review_log.APPROVED], 2)
        TypingLog(self.userdata / review_log.LOG_NAME).write(["ㄐㄧㄚ"], "家", "加", [(0, 1, "加")], "", "casual", False)
        self.assertEqual(st.import_new(), 1)            # works again after the log was emptied

    def test_mark_changes(self):
        self.assertEqual(review_log.mark_changes("我在家", "我再家"), "我【再】家")
        self.assertEqual(review_log.mark_changes("我在", "我在"), "我在")


if __name__ == "__main__":
    unittest.main()
