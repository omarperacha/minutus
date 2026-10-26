"""The temporary maintenance exception rejects expiry and dependency drift."""
from copy import deepcopy
from datetime import date
import unittest
from check_dependency_policy import validate_paste_exception


def metadata():
    return {'packages': [
        {'id': 'paste', 'name': 'paste', 'version': '1.0.15', 'targets': [{'kind': ['proc-macro']}]},
        {'id': 'tokenizers', 'name': 'tokenizers', 'version': '0.22.2', 'targets': [{'kind': ['lib']}]},
    ], 'resolve': {'nodes': [{'id': 'tokenizers', 'deps': [{'pkg': 'paste'}]}]}}


class TemporaryPasteExceptionTests(unittest.TestCase):
    def test_current_compile_time_dependency_is_accepted(self):
        for today in [date(2026, 10, 13), date(2026, 11, 13), date(2027, 10, 12)]:
            with self.subTest(today=today):
                validate_paste_exception(metadata(), today)

    def test_deadline_and_later_dates_are_rejected(self):
        for today in [date(2027, 10, 13), date(2028, 1, 1)]:
            with self.subTest(today=today), self.assertRaisesRegex(ValueError, 'expired'):
                validate_paste_exception(metadata(), today)

    def test_tokenizer_upgrades_require_review(self):
        value = metadata()
        value['packages'][1]['version'] = '0.23.2'
        with self.assertRaisesRegex(ValueError, 'dependency path changed'):
            validate_paste_exception(value, date(2026, 10, 13))

    def test_additional_direct_users_require_review(self):
        value = metadata()
        value['packages'].append({'id': 'another', 'name': 'another', 'version': '1.0.0'})
        value['resolve']['nodes'].append({'id': 'another', 'deps': [{'pkg': 'paste'}]})
        with self.assertRaisesRegex(ValueError, 'dependency path changed'):
            validate_paste_exception(value, date(2026, 10, 13))

    def test_paste_upgrades_or_additional_versions_require_review(self):
        for extra in [False, True]:
            value = metadata()
            changed = deepcopy(value['packages'][0])
            changed['version'] = '1.0.16'
            if extra:
                changed['id'] = 'paste-new'
                value['packages'].append(changed)
            else:
                value['packages'][0] = changed
            with self.subTest(extra=extra), self.assertRaisesRegex(ValueError, 'version changed'):
                validate_paste_exception(value, date(2026, 10, 13))

    def test_runtime_library_is_not_covered(self):
        value = metadata()
        value['packages'][0]['targets'] = [{'kind': ['lib']}]
        with self.assertRaisesRegex(ValueError, 'proc-macro'):
            validate_paste_exception(value, date(2026, 10, 13))

    def test_obsolete_exception_requires_removal(self):
        value = metadata()
        value['packages'] = value['packages'][1:]
        with self.assertRaisesRegex(ValueError, 'obsolete'):
            validate_paste_exception(value, date(2026, 10, 13))


if __name__ == '__main__':
    unittest.main()
