"""The fixture guard of tests/chatfix.py refuses the owner's real shared skills.

The audit hook decides what is a path the way `os.fspath` does: anything it accepts is checked and
anything it refuses (a TypeError) is not a path. An `isinstance(os.PathLike)` shortcut is not the
same rule, because the ABC remembers a negative answer even after a class gains `__fspath__`."""

import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import chatfix

GUARDED_EVENT = "os.listdir"


class Hook(unittest.TestCase):
    def setUp(self) -> None:
        chatfix.guard(self)  # active until the test is over

    def refused(self, value: object) -> None:
        with self.assertRaises(chatfix.ForbiddenFallback):
            chatfix._hook(GUARDED_EVENT, (value,))

    def test_01_the_real_skills_are_refused_as_text_bytes_or_a_path(self) -> None:
        real = chatfix.REAL_SKILLS / "chat"
        self.refused(str(real))
        self.refused(os.fsencode(real))
        self.refused(real)
        self.refused(chatfix.REAL_SKILLS)

    def test_02_other_paths_and_values_that_are_no_path_are_ignored(self) -> None:
        for value in (Path("/tmp"), "relative/name", 3, None, object(), [chatfix.REAL_SKILLS]):
            chatfix._hook(GUARDED_EVENT, (value,))

    def test_03_only_the_guarded_events_are_checked(self) -> None:
        chatfix._hook("os.getcwd", (chatfix.REAL_SKILLS,))

    def test_04_a_path_object_whose_fspath_appears_late_is_still_refused(self) -> None:
        late_class = type("LatePath", (), {})
        value = late_class()
        # The ABC caches this negative answer; gaining __fspath__ afterwards does not undo it.
        self.assertNotIsInstance(value, os.PathLike)
        type.__setattr__(late_class, "__fspath__", lambda self: str(chatfix.REAL_SKILLS / "chat"))
        self.assertNotIsInstance(value, os.PathLike)
        self.assertEqual(os.fspath(value), str(chatfix.REAL_SKILLS / "chat"))
        self.refused(value)


class Released(unittest.TestCase):
    def test_05_without_an_active_guard_nothing_is_refused(self) -> None:
        chatfix._hook(GUARDED_EVENT, (str(chatfix.REAL_SKILLS),))


if __name__ == "__main__":
    unittest.main()
