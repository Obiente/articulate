import hashlib
import tempfile
import unittest
from pathlib import Path
from generate import correction_state, generate, shortcut_state
from train import correction_allowed, shortcut_allowed


class PipelineTests(unittest.TestCase):
    def test_seeded_generation_is_byte_reproducible(self):
        with tempfile.TemporaryDirectory() as directory:
            left, right = Path(directory)/'left', Path(directory)/'right'
            generate(left, 42); generate(right, 42)
            hashes = lambda root: {p.relative_to(root): hashlib.sha256(p.read_bytes()).hexdigest()
                                   for p in root.rglob('*') if p.is_file()}
            self.assertEqual(hashes(left), hashes(right))

    def test_unconfirmed_and_wrong_scope_stay_unchanged(self):
        for confirmed, scope in [(False,True),(True,False),(False,False)]:
            self.assertFalse(correction_allowed(correction_state('at Casey','@Casey',confirmed,scope,'Chat')))
        self.assertTrue(correction_allowed(correction_state('at Casey','@Casey',True,True,'Chat')))

    def test_protected_numbers_negations_and_currency_override_any_model_score(self):
        for before, after in [('do not send','do send'),('never delete','delete'),
                              ('costs 120','costs 12'),('costs -120','costs 120'),
                              ('two files','three files'),('costs $20','costs €20'),
                              ('rate 5%','rate 5'),('without access','with access')]:
            with self.subTest(before=before):
                self.assertFalse(correction_allowed(correction_state(before,after,True,True,'Chat')))

    def test_only_focused_exact_complete_prefix_can_expand(self):
        for utterance in ['bang','bang sig','I mentioned bang signature','bang signatured','bang unknown']:
            self.assertFalse(shortcut_allowed(shortcut_state('bang signature',utterance)))
        self.assertTrue(shortcut_allowed(shortcut_state('bang signature','bang signature Hello.')))
        self.assertFalse(shortcut_allowed(shortcut_state('bang signature','bang signature Hello.',False)))


if __name__=='__main__': unittest.main()
