import importlib.util
import json
import subprocess
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location(
    "analyze_gemini_usage", ROOT / "scripts" / "analyze-gemini-usage.py"
)
USAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(USAGE)


class UsageTests(unittest.TestCase):
    def test_events_and_summaries_are_not_added_twice(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a session=100 usage_samples=1 "
                "prompt_tokens=7\n",
                "codetrial live_turn_usage room=b usage_samples=1 prompt_tokens=90\n",
                "codetrial live_usage room=a session=100 model=live elapsed_s=60 "
                "usage_samples=1 prompt_tokens=7\n",
                "codetrial live_usage room=b usage_samples=1 prompt_tokens=90\n",
                "gemini report room=a call=1 retry=0 usage "
                "usage_samples=1 prompt_tokens=30\n",
                "gemini report room=a call=2 retry=1 usage "
                "usage_samples=1 prompt_tokens=30\n",
            ]
        )
        self.assertEqual(len(report["rooms"]), 2)
        room = report["rooms"][0]
        self.assertEqual(room["live"]["observed_token_sums"]["prompt_tokens"], 7)
        self.assertEqual(room["live"]["excluded_event_records"], 1)
        session = room["live_sessions"][0]
        self.assertEqual(session["session"], "100")
        self.assertEqual(session["model"], "live")
        self.assertEqual(session["source"], "session_summary")
        self.assertEqual(room["http"]["report"]["records"], 2)
        self.assertEqual(
            room["http"]["report"]["observed_token_sums"]["prompt_tokens"], 60
        )
        self.assertEqual(report["warnings"], [])

    def test_unknown_usage_and_partial_details_are_not_complete(self):
        report = USAGE.analyze(
            [
                "gemini interim room=a call=1 retry=0 usage usage_samples=0 "
                "prompt_tokens=0 prompt_detail_samples=0\n",
                "codetrial live_usage room=a usage_samples=2 "
                "prompt_tokens=200 prompt_detail_samples=1 prompt_audio_tokens=70\n",
            ]
        )
        room = report["rooms"][0]
        self.assertEqual(room["http"]["interim"]["records_without_known_usage"], 1)
        self.assertEqual(
            room["live"]["modality_coverage"]["prompt"], "partial_or_unknown"
        )

    def test_missing_summary_and_legacy_records_remain_unknown(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a usage_samples=1 prompt_tokens=10\n",
                "codetrial live_usage turns=1 prompt_tokens=30\n",
            ]
        )
        self.assertTrue(report["rooms"][0]["live"]["incomplete"])
        self.assertEqual(report["rooms"][1]["room"], "unknown")
        self.assertEqual(report["rooms"][1]["live"]["records_without_known_usage"], 1)
        self.assertEqual(len(report["warnings"]), 1)

    def test_filter_and_empty_input(self):
        lines = [
            "codetrial live_usage room=a usage_samples=1 prompt_tokens=10\n",
            "codetrial live_usage room=b usage_samples=1 prompt_tokens=20\n",
        ]
        self.assertEqual(USAGE.analyze(lines, "b")["rooms"][0]["room"], "b")
        self.assertEqual(USAGE.analyze(["unrelated content"])["rooms"], [])

    def test_cli_refuses_empty_measurements_without_echoing_input(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "analyze-gemini-usage.py")],
            input="unrelated private interview content\n",
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertEqual(json.loads(result.stdout)["rooms"], [])
        self.assertNotIn("private interview", result.stdout + result.stderr)

    def test_complete_details_require_both_sample_and_token_coverage(self):
        record = (
            "codetrial live_usage room=a usage_samples=1 prompt_tokens=70 "
            "prompt_detail_samples=1 prompt_audio_tokens=70 prompt_text_tokens=0 "
            "prompt_image_tokens=0 prompt_video_tokens=0 prompt_other_tokens=0\n"
        )
        complete = USAGE.analyze([record])["rooms"][0]["live"]
        self.assertEqual(complete["modality_coverage"]["prompt"], "complete")
        partial = USAGE.analyze(
            [record.replace("prompt_tokens=70", "prompt_tokens=80")]
        )
        self.assertEqual(
            partial["rooms"][0]["live"]["modality_coverage"]["prompt"],
            "partial_or_unknown",
        )

    def test_incomplete_event_capture_is_reported(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a usage_samples=1 prompt_tokens=10\n",
                "codetrial live_usage room=a usage_samples=2 prompt_tokens=30\n",
            ]
        )
        self.assertEqual(len(report["warnings"]), 1)
        self.assertEqual(
            report["rooms"][0]["live"]["observed_token_sums"]["prompt_tokens"], 30
        )

    def test_log_prefix_is_matched_but_cannot_inject_fields(self):
        report = USAGE.analyze(
            [
                "2026-10-01T03:00:00 host codetrial[12]: room=evil prompt_tokens=999 "
                "codetrial live_usage room=a session=1 usage_samples=1 "
                "prompt_tokens=5\n",
                "Oct 01 host app[3]: gemini report room=a call=1 retry=0 usage "
                "usage_samples=1 prompt_tokens=4\n",
            ]
        )
        self.assertEqual([r["room"] for r in report["rooms"]], ["a"])
        room = report["rooms"][0]
        self.assertEqual(room["live"]["observed_token_sums"]["prompt_tokens"], 5)
        self.assertEqual(
            room["http"]["report"]["observed_token_sums"]["prompt_tokens"], 4
        )

    def test_unfinished_second_session_in_room_is_reported(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a session=1 usage_samples=1 "
                "prompt_tokens=10\n",
                "codetrial live_usage room=a session=1 model=m elapsed_s=5 "
                "outcome=ok sockets=1 usage_samples=1 prompt_tokens=10\n",
                "codetrial live_turn_usage room=a session=2 usage_samples=1 "
                "prompt_tokens=20\n",
            ]
        )
        room = report["rooms"][0]
        first, second = room["live_sessions"]
        self.assertEqual(first["session"], "1")
        self.assertEqual(first["outcome"], "ok")
        self.assertFalse(first["incomplete"])
        self.assertEqual(first["excluded_event_records"], 1)
        self.assertEqual(second["session"], "2")
        self.assertEqual(second["source"], "events_without_summary")
        self.assertTrue(second["incomplete"])
        self.assertEqual(second["observed_token_sums"]["prompt_tokens"], 20)
        self.assertEqual(room["live"]["observed_token_sums"]["prompt_tokens"], 30)
        self.assertTrue(room["live"]["incomplete"])
        self.assertEqual(room["live"]["sessions"], 2)
        self.assertEqual(len(report["warnings"]), 1)

    def test_events_without_a_session_are_not_added_to_a_summary(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a usage_samples=1 prompt_tokens=7\n",
                "codetrial live_usage room=a session=100 usage_samples=1 "
                "prompt_tokens=7\n",
            ]
        )
        room = report["rooms"][0]
        self.assertEqual([s["session"] for s in room["live_sessions"]], ["100"])
        self.assertEqual(room["live"]["observed_token_sums"]["prompt_tokens"], 7)
        self.assertEqual(room["live"]["excluded_event_records"], 1)
        self.assertFalse(room["live"]["incomplete"])

    def test_snapshot_samples_warn_and_report_turn_complete_bound(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a session=1 usage_samples=1 "
                "turn_complete_samples=0 prompt_tokens=50\n",
                "codetrial live_turn_usage room=a session=1 usage_samples=1 "
                "turn_complete_samples=1 prompt_tokens=60\n",
            ]
        )
        session = report["rooms"][0]["live_sessions"][0]
        self.assertEqual(session["observed_token_sums"]["prompt_tokens"], 110)
        self.assertEqual(session["turn_complete_only_sums"]["prompt_tokens"], 60)
        self.assertTrue(any("periodic snapshots" in w for w in report["warnings"]))
        matched = USAGE.analyze(
            [
                "codetrial live_usage room=a session=1 usage_samples=2 "
                "turn_complete_samples=2 prompt_tokens=60\n",
            ]
        )
        self.assertEqual(matched["warnings"], [])

    def test_an_empty_direction_is_none_reported_never_complete(self):
        report = USAGE.analyze(
            [
                "codetrial live_usage room=a session=1 usage_samples=1 "
                "prompt_tokens=70 prompt_detail_samples=1 prompt_audio_tokens=70 "
                "prompt_text_tokens=0 prompt_image_tokens=0 prompt_video_tokens=0 "
                "prompt_other_tokens=0 tool_use_prompt_tokens=0 "
                "tool_use_detail_samples=0 cached_tokens=40 cache_detail_samples=1 "
                "cache_text_tokens=40 cache_audio_tokens=0 cache_image_tokens=0 "
                "cache_video_tokens=0 cache_other_tokens=0\n",
            ]
        )
        live = report["rooms"][0]["live"]
        self.assertEqual(live["modality_coverage"]["tool_use"], "none_reported")
        self.assertEqual(live["modality_coverage"]["cache"], "complete")
        self.assertEqual(live["observed_token_sums"]["cache_text_tokens"], 40)
        missing = USAGE.analyze(
            ["codetrial live_usage room=a usage_samples=1 cached_tokens=40\n"]
        )
        self.assertEqual(
            missing["rooms"][0]["live"]["modality_coverage"]["cache"],
            "partial_or_unknown",
        )

    def test_phase_judge_calls_and_skips_are_counted(self):
        report = USAGE.analyze(
            [
                "gemini phase room=team:a call=1 retry=0 usage usage_samples=1 "
                "prompt_tokens=40\n",
                "phase judge skipped room=team:a: quota\n",
            ]
        )
        room = report["rooms"][0]
        self.assertEqual(room["room"], "team:a")
        self.assertEqual(room["http"]["phase"]["records"], 1)
        self.assertEqual(room["http_failures"]["phase_judge_skipped"], 1)

    def test_key_moment_calls_and_skips_are_counted(self):
        report = USAGE.analyze(
            [
                "gemini moments room=team:a call=1 retry=0 usage usage_samples=1 "
                "prompt_tokens=40\n",
                "key moments skipped room=team:a: quota\n",
            ]
        )
        room = report["rooms"][0]
        self.assertEqual(room["http"]["moments"]["records"], 1)
        self.assertEqual(room["http_failures"]["key_moments_skipped"], 1)

    def test_failure_lines_keep_a_room_name_with_a_colon(self):
        report = USAGE.analyze(
            [
                "codetrial live_usage room=team:a session=1 usage_samples=1 "
                "prompt_tokens=5\n",
                "gemini report transport_failed room=team:a call=1 final=true "
                "error=x\n",
                "interim review skipped room=team:a: Gemini billing or prepaid "
                "credit exhausted\n",
            ]
        )
        self.assertEqual([room["room"] for room in report["rooms"]], ["team:a"])
        self.assertEqual(
            report["rooms"][0]["http_failures"],
            {
                "report_transport_failed": 1,
                "interim_skipped": 1,
                "phase_judge_skipped": 0,
                "key_moments_skipped": 0,
            },
        )

    def test_context_curve_uses_turn_complete_observations(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a session=1 at=00:01.000 socket=1 "
                "cause=candidate usage_samples=1 turn_complete_samples=1 "
                "prompt_tokens=100 response_tokens=5\n",
                "codetrial live_turn_usage room=a session=1 at=00:02.000 socket=1 "
                "cause=watch usage_samples=1 turn_complete_samples=0 "
                "prompt_tokens=900\n",
                "codetrial live_turn_usage room=a session=1 at=00:03.000 socket=2 "
                "cause=tool usage_samples=1 turn_complete_samples=1 "
                "prompt_tokens=400 response_tokens=7\n",
                "codetrial live_turn_usage room=a session=1 at=00:04.000 socket=2 "
                "cause=candidate usage_samples=1 turn_complete_samples=1 "
                "prompt_tokens=250 response_tokens=1\n",
            ]
        )
        session = report["rooms"][0]["live_sessions"][0]
        self.assertEqual(
            session["context_curve"][1],
            {"at": "00:03.000", "socket": 2, "cause": "tool", "prompt_tokens": 400},
        )
        self.assertEqual(len(session["context_curve"]), 3)
        growth = session["context_growth"]
        self.assertEqual(growth["first"], 100)
        self.assertEqual(growth["max"], 400)
        self.assertEqual(growth["last"], 250)
        self.assertEqual(growth["completed_observations"], 3)
        # Only socket 2 has a step, 400 to 250; socket 1 has one point.
        self.assertEqual(growth["mean_growth_per_observation"], -150.0)
        single = USAGE.analyze(
            ["codetrial live_turn_usage room=a usage_samples=1 prompt_tokens=9\n"]
        )["rooms"][0]["live_sessions"][0]
        self.assertEqual(single["context_growth"]["basis"], "all_events")
        self.assertIsNone(single["context_growth"]["mean_growth_per_observation"])

    def test_causes_are_broken_down(self):
        report = USAGE.analyze(
            [
                "codetrial live_turn_usage room=a session=1 cause=watch "
                "usage_samples=1 prompt_tokens=10 response_tokens=1\n",
                "codetrial live_turn_usage room=a session=1 cause=watch "
                "usage_samples=1 prompt_tokens=20 response_tokens=2\n",
                "codetrial live_turn_usage room=a session=1 "
                "usage_samples=1 prompt_tokens=5\n",
            ]
        )
        causes = report["rooms"][0]["live_sessions"][0]["by_cause"]
        self.assertEqual(
            causes["watch"],
            {"observations": 2, "prompt_tokens": 30, "response_tokens": 3},
        )
        self.assertEqual(causes["unknown"]["observations"], 1)

    def test_http_failures_and_context_refreshes_are_counted(self):
        lines = [
            "gemini report transport_failed room=a call=1 backoff_s=2 "
            "error=secret prompt_tokens=5\n",
            "gemini report transport_failed room=a call=2 backoff_s=4 error=x\n",
            "gemini report retry_unavailable room=a call=2\n",
            "interim review skipped room=a: quota\n",
            "codetrial context_refresh room=a session=1 bytes=300\n",
            "codetrial context_refresh room=a session=1 bytes=200\n",
            "codetrial context_refresh room=a bytes=50\n",
            "codetrial live_usage room=a session=1 usage_samples=1 prompt_tokens=5\n",
        ]
        room = USAGE.analyze(lines)["rooms"][0]
        self.assertEqual(
            room["http_failures"],
            {
                "report_transport_failed": 2,
                "interim_skipped": 1,
                "phase_judge_skipped": 0,
                "key_moments_skipped": 0,
            },
        )
        self.assertNotIn("http", room)
        self.assertEqual(
            room["live_sessions"][0]["context_refreshes"], {"count": 2, "bytes": 500}
        )
        self.assertEqual(room["context_refreshes"], {"count": 1, "bytes": 50})
        other = USAGE.analyze(lines, "b")
        self.assertEqual(other["rooms"], [])
        self.assertEqual(other["usage_records"], 0)

    def test_cli_exits_2_when_only_failures_match(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "analyze-gemini-usage.py")],
            input="gemini report transport_failed room=a call=1 backoff_s=1 "
            "error=private detail\n"
            "codetrial context_refresh room=a session=1 bytes=10\n",
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 2)
        output = json.loads(result.stdout)
        self.assertEqual(output["usage_records"], 0)
        self.assertEqual(
            output["rooms"][0]["http_failures"]["report_transport_failed"], 1
        )
        self.assertNotIn("private", result.stdout + result.stderr)

    def test_zero_usage_summary_does_not_spoil_coverage(self):
        full = (
            "prompt_detail_samples=1 prompt_text_tokens=5 prompt_audio_tokens=0 "
            "prompt_image_tokens=0 prompt_video_tokens=0 prompt_other_tokens=0"
        )
        report = USAGE.analyze(
            [
                f"codetrial live_usage room=a session=1 usage_samples=1 "
                f"prompt_tokens=5 {full}\n",
                "codetrial live_usage room=a session=2 outcome=billing sockets=0 "
                "usage_samples=0 prompt_tokens=0 prompt_detail_samples=0\n",
            ]
        )
        live = report["rooms"][0]["live"]
        self.assertEqual(live["records_without_known_usage"], 1)
        self.assertEqual(live["modality_coverage"]["prompt"], "complete")

    def test_context_growth_is_measured_within_each_socket(self):
        lines = [
            f"codetrial live_turn_usage room=a session=1 socket={socket} "
            f"usage_samples=1 turn_complete_samples=1 prompt_tokens={prompt}\n"
            for socket, prompt in ((1, 1000), (1, 3000), (2, 500), (2, 1500))
        ]
        growth = USAGE.analyze(lines)["rooms"][0]["live_sessions"][0]["context_growth"]
        self.assertEqual(growth["mean_growth_per_observation"], 1500.0)


if __name__ == "__main__":
    unittest.main()
