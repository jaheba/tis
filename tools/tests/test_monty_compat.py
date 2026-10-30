import contextlib
import importlib.util
import io
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("check_monty", Path(__file__).parents[1] / "check_monty.py")
compat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compat)


class FixtureTests(unittest.TestCase):
    def test_header_keeps_nested_comments(self):
        source = "# /// monty\n# mount-fs = true\n# # explanatory comment\n# ///\npass\n"
        self.assertEqual(compat.header(source), {"mount-fs": True})
        self.assertEqual(compat.prepare(source, "x.py")[1]["status"], "outside_scope")

    def test_multiline_literals_and_unicode_are_unchanged(self):
        source = 'value = """雪\\n\n    second"""\nvalue\n# Return.str=unused\n'
        code, info = compat.prepare(source, "x.py")
        namespace = {}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            exec(code, namespace)
        self.assertEqual(namespace["value"], "雪\n\n    second")
        self.assertIn(info["marker"] + "return:雪\n\n    second", out.getvalue())

    def test_terminal_expression_is_evaluated_once(self):
        source = "calls=[]\ndef f():\n    calls.append(1)\n    return 42\nf()\n# Return=42\n"
        code, info = compat.prepare(source, "x.py")
        namespace = {}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            exec(code, namespace)
        self.assertEqual(namespace["calls"], [1])
        self.assertEqual(compat.classify(info, 0, out.getvalue(), "")["status"], "pass")

    def test_no_marker_is_not_a_pass(self):
        _, info = compat.prepare("assert True\n", "x.py")
        self.assertEqual(compat.classify(info, 0, "", "")["status"], "harness_unverified")

    def test_traceback_type_is_checked(self):
        info = {"marker": "mark", "expectation": "traceback", "expected": "TRACEBACK:\nValueError: bad"}
        self.assertEqual(
            compat.classify(info, 1, "", "type error: bad")["status"], "traceback_type_difference"
        )
        self.assertEqual(compat.classify(info, 1, "", "ValueError: bad")["status"], "traceback_unverified")

    def test_message_difference_is_not_a_pass(self):
        info = {"marker": "mark", "expectation": "raise", "expected": "ValueError('one')"}
        self.assertEqual(
            compat.classify(info, 1, "", "ValueError: two")["status"], "exception_message_difference"
        )


if __name__ == "__main__":
    unittest.main()
