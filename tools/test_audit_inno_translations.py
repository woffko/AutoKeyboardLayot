import unittest
from audit_inno_translations import compare, decode, parameters, parse


class TranslationAuditTests(unittest.TestCase):
    def test_parameter_order_and_escaped_percent(self):
        self.assertEqual(parameters('%2 then %1'), parameters('%1 then %2'))
        self.assertEqual(parameters('%%1'), {})
        self.assertNotEqual(parameters('[name] %1'), parameters('[name/ver] %1'))
        repeated = compare(parse('[Messages]\nTest=%1\n'), parse('[Messages]\nTest=%1 then %1\n'))
        self.assertEqual(repeated['parameter_mismatches'], [])

    def test_missing_blank_and_bad_parameters_are_reported(self):
        reference = parse('[Messages]\nFirst=%1 [name]\nSecond=Text\nOptional=\n')
        candidate = parse('[Messages]\nFirst=%2 [name]\nSecond=\nExtra=Other\n')
        result = compare(reference, candidate)
        self.assertEqual(result['missing_messages'], ['Second'])
        self.assertEqual(result['parameter_mismatches'], ['First'])
        self.assertEqual(result['extra_messages'], ['Extra'])

    def test_rejects_code_and_duplicate_keys_and_decodes_utf16(self):
        for text in ('[Code]\nSomething=1', '[Messages]\nName=1\nName=2'):
            with self.assertRaises(ValueError):
                parse(text)
        text = '[Messages]\nName=हिन्दी\n'
        self.assertEqual(decode(text.encode('utf-16'))[0], text)


if __name__ == '__main__':
    unittest.main()
