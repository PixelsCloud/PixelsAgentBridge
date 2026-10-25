import importlib.util
from pathlib import Path
import unittest

spec=importlib.util.spec_from_file_location('proof',Path(__file__).resolve().parents[1]/'remote_desktop_proof.py')
proof=importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class ProofTests(unittest.TestCase):
    def sample(self):
        captures=[]
        events=[dict(event='opened',pattern_proof=True)]
        for frame in range(1,13):
            captures.append(dict(frame=frame,pattern=frame+120,checks=576,max_error=0,fingerprint=str(frame)))
            events.extend([dict(event='received',frame=frame,fingerprint=str(frame)),
                dict(event='presented',frame=frame,native_timings=dict(pattern_proof=dict(pattern=frame+120,checks=1152,decoded_error=2,rendered_error=3)))])
        events.append(dict(event='closed',native_summary=dict(test_pixel_checks=12*1152,decoder='d3d11_hardware',displayed=12)))
        events.append(dict(event='telemetry',written_events=len(events),dropped_events=0))
        return events,captures

    def test_complete_dynamic_proof_passes(self):
        self.assertEqual(proof.analyze(*self.sample())['status'],'passed')

    def test_stale_correctly_colored_frame_is_detected_by_source_marker(self):
        events,captures=self.sample()
        events[2]['native_timings']['pattern_proof']['pattern']-=1
        self.assertIn('another source frame',' '.join(proof.analyze(events,captures)['errors']))

    def test_packet_mismatch_is_not_hidden_by_valid_display(self):
        events,captures=self.sample()
        events[1]['fingerprint']='different'
        self.assertIn('fingerprint mismatch',' '.join(proof.analyze(events,captures)['errors']))

    def test_coalesced_proof_or_missing_tail_cannot_pass(self):
        events,captures=self.sample()
        events.pop(2)
        self.assertEqual(proof.analyze(events,captures)['status'],'failed')
        events,captures=self.sample()
        self.assertEqual(proof.analyze(events[:-1],captures)['status'],'failed')

    def test_unverified_pixels_and_unobserved_relay_cannot_pass(self):
        events,captures=self.sample()
        events[0]['relay_requested']=True
        captures[0]['max_error']=1
        errors=' '.join(proof.analyze(events,captures)['errors'])
        self.assertIn('capture source',errors)
        self.assertIn('Relay',errors)

    def test_hardware_qualification_requires_explicit_backend_evidence(self):
        events,captures=self.sample()
        self.assertEqual(proof.analyze(events,captures,'intel_mf_gpu')['status'],'failed')
        for row in captures:row['encoder']='intel_mf_gpu'
        self.assertEqual(proof.analyze(events,captures,'intel_mf_gpu')['status'],'passed')

    def test_software_fallback_cannot_be_attributed_to_hardware(self):
        events,captures=self.sample()
        for row in captures:row['encoder']='intel_mf_gpu'
        captures[5]['encoder']='openh264_software'
        result=proof.analyze(events,captures,'intel_mf_gpu')
        self.assertEqual(result['status'],'failed')
        self.assertEqual(result['encoders'],['intel_mf_gpu','openh264_software'])


if __name__=='__main__':unittest.main()
