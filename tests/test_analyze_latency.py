import importlib.util
import json
import random
import re
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/analyze-latency.py"
SPEC = importlib.util.spec_from_file_location("analyze_latency", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def line(**fields):
    return "Oct 14 agent[12]: codetrial latency " + json.dumps(
        {
            "event": "reply_latency",
            "version": 2,
            "pid": 7,
            "room": "r",
            "session": 1,
            "socket": 1,
            "origin_socket": 1,
            "turn": 1,
            "origin": "last_voiced_pcm",
            "silence_window_ms": 3_000,
            **fields,
        }
    )


def captured(**fields):
    return line(
        stage="native_audio_capture",
        captured=True,
        **{
            "elapsed_ms": 3_700,
            "provider_voice_end_ms": 3_100,
            "first_provider_output_ms": 3_500,
            "output_queue_ms": 20,
            "native_capture_ms": 10,
            "server_total_ms": 3_730,
            **fields,
        },
    )


def run(*args, stdin):
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        input=stdin,
        text=True,
        capture_output=True,
        check=False,
    )


class WaterfallTests(unittest.TestCase):
    def test_plain_reply_partitions_the_server_total(self):
        row = MODULE.analyze([captured()])["turns"][0]
        self.assertEqual(
            row["waterfall_ms"],
            {
                "silence_window": 3_000,
                "endpointing": 100,
                "response_start": 400,
                "output_to_audio": 200,
                "output_queue": 20,
                "native_capture": 10,
                "unattributed": 0,
            },
        )
        self.assertEqual(row["beyond_window_ms"], 730)
        self.assertEqual(row["dominant"], "response_start")

    def test_an_end_sent_with_the_first_output_still_shows_the_wait(self):
        row = MODULE.analyze(
            [captured(provider_voice_end_ms=3_500, first_provider_output_ms=3_500)]
        )["turns"][0]
        self.assertEqual(row["waterfall_ms"]["endpointing"], 500)
        self.assertEqual(row["waterfall_ms"]["response_start"], 0)
        self.assertEqual(row["dominant"], "endpointing")

    def test_the_window_applies_only_after_voiced_input(self):
        row = MODULE.analyze([captured(origin="prompt", provider_voice_end_ms=None)])[
            "turns"
        ][0]
        self.assertNotIn("silence_window", row["waterfall_ms"])
        self.assertEqual(row["beyond_window_ms"], 3_730)

    def test_handler_split_comes_from_the_accepted_record(self):
        row = MODULE.analyze(
            [
                line(stage="audio_accepted", elapsed_ms=3_700, handler_tail_ms=40),
                captured(),
            ]
        )["turns"][0]
        self.assertEqual(row["first_audio"]["handler_tail_ms"], 40)
        self.assertEqual(row["first_audio"]["stage"], "native_audio_capture")

    def test_a_yield_inside_the_window_replaces_it(self):
        row = MODULE.analyze(
            [
                captured(
                    candidate_yield_ms=800,
                    provider_voice_end_ms=1_200,
                    first_provider_output_ms=1_200,
                    elapsed_ms=1_400,
                    server_total_ms=1_430,
                )
            ]
        )["turns"][0]
        self.assertEqual(row["waterfall_ms"]["candidate_yield"], 800)
        self.assertEqual(row["waterfall_ms"]["endpointing"], 400)
        self.assertNotIn("silence_window", row["waterfall_ms"])
        self.assertEqual(row["beyond_window_ms"], 630)
        self.assertEqual(row["dominant"], "endpointing")

    def test_a_yield_after_the_end_of_speech_changes_nothing(self):
        row = MODULE.analyze([captured(candidate_yield_ms=3_300)])["turns"][0]
        self.assertNotIn("candidate_yield", row["waterfall_ms"])
        self.assertEqual(row["waterfall_ms"]["silence_window"], 3_000)

    def test_tool_turn_splits_decision_rounds_and_continuation(self):
        row = MODULE.analyze(
            [
                line(stage="tool_response", elapsed_ms=3_900, tool_batch_ms=40),
                captured(
                    elapsed_ms=5_000,
                    first_tool_call_ms=3_400,
                    last_tool_response_ms=3_900,
                    first_provider_output_ms=3_400,
                    tool_batches=1,
                    tool_handler_ms=40,
                    server_total_ms=5_030,
                ),
            ]
        )["turns"][0]
        self.assertEqual(row["waterfall_ms"]["tool_decision"], 300)
        self.assertEqual(row["waterfall_ms"]["tool_rounds"], 500)
        self.assertEqual(row["waterfall_ms"]["post_tool_response"], 1_100)
        self.assertNotIn("response_start", row["waterfall_ms"])
        self.assertEqual(row["dominant"], "post_tool_response")
        self.assertEqual(
            [e["stage"] for e in row["timeline"]],
            ["tool_response", "native_audio_capture"],
        )

    def test_tool_names_and_transcript_lead_are_summarized(self):
        report = MODULE.analyze(
            [
                line(
                    stage="tool_response",
                    elapsed_ms=3_900,
                    tool_names=["record_framework_evidence", "log_hint"],
                    sent=True,
                ),
                line(
                    stage="tool_response",
                    elapsed_ms=3_950,
                    tool_names=["log_hint"],
                    sent=False,
                ),
                captured(first_transcript_ms=1_100),
            ]
        )
        self.assertEqual(
            report["tool_calls"], {"record_framework_evidence": 1, "log_hint": 1}
        )
        self.assertEqual(report["transcript_before_end"]["p50_ms"], 2_000)
        self.assertEqual(
            MODULE.analyze([line(stage="turn_complete", tool_names="x")])[
                "rejected_records"
            ],
            1,
        )

    def test_context_size_is_optional_but_must_be_a_count(self):
        self.assertEqual(
            MODULE.analyze([captured(context_tokens=None)])["rejected_records"], 0
        )
        self.assertEqual(
            MODULE.analyze([captured(context_tokens=5_000)])["rejected_records"], 0
        )
        self.assertEqual(
            MODULE.analyze([captured(context_tokens="big")])["rejected_records"], 1
        )

    def test_audio_before_a_tool_call_is_not_a_tool_turn(self):
        row = MODULE.analyze(
            [captured(first_tool_call_ms=4_000, last_tool_response_ms=4_100)]
        )["turns"][0]
        self.assertIn("response_start", row["waterfall_ms"])

    def test_an_end_after_output_is_not_endpointing(self):
        row = MODULE.analyze([captured(provider_voice_end_ms=3_600)])["turns"][0]
        self.assertNotIn("endpointing", row["waterfall_ms"])
        self.assertEqual(row["waterfall_ms"]["response_start"], 500)


class AggregateTests(unittest.TestCase):
    def test_reply_time_is_banded_and_sloped_by_context_size(self):
        report = MODULE.analyze(
            [
                captured(turn=1, context_tokens=4_000),
                captured(turn=2, context_tokens=12_000, server_total_ms=3_830),
                captured(turn=3, context_tokens=20_000, server_total_ms=3_930),
                captured(turn=4, context_tokens=None),
            ]
        )
        self.assertEqual(
            {
                band: stats["count"]
                for band, stats in report["beyond_window_by_context"].items()
            },
            {"0k_8k": 1, "8k_16k": 1, "16k_32k": 1},
        )
        self.assertEqual(
            list(report["beyond_window_by_context"]), ["0k_8k", "8k_16k", "16k_32k"]
        )
        self.assertEqual(report["context_slope_ms_per_1k"], 12.5)
        summary = MODULE.summary_lines(report)
        self.assertIn("context_8k_16k=n1 p50=830 p95=830 small_n", summary)
        self.assertIn("context_slope_ms_per_1k=12.5", summary)

    def test_a_flat_or_thin_context_sample_has_no_slope(self):
        self.assertIsNone(MODULE.slope_per_1k([("a", 1_000, 5), ("a", 2_000, 6)]))
        self.assertIsNone(
            MODULE.slope_per_1k([("a", 1_000, 5), ("a", 1_000, 6), ("a", 1_000, 7)])
        )
        # Three points, but the lone "b" only fixes its own mean: two left.
        self.assertIsNone(
            MODULE.slope_per_1k([("a", 1_000, 5), ("a", 2_000, 6), ("b", 9_000, 40)])
        )
        self.assertEqual(MODULE.context_band(64_000), "64k_up")

    def test_the_context_slope_is_fitted_within_each_kind_of_reply(self):
        # Tool replies are slower and come later, but neither kind slows as
        # its own context grows: a pooled fit would call that mix growth.
        points = [
            ("no_tool", 4_000, 600),
            ("no_tool", 8_000, 600),
            ("tool", 20_000, 1_300),
            ("tool", 24_000, 1_300),
        ]
        self.assertEqual(MODULE.slope_per_1k(points), 0.0)

    def test_percentiles_interpolate_and_a_small_sample_says_so(self):
        stats = MODULE.statistics(list(range(1, 21)))
        # Half a rank is kept, not rounded to the even neighbour.
        self.assertEqual(stats["p50_ms"], 10.5)
        self.assertEqual(stats["p95_ms"], 19.1)
        self.assertFalse(stats["small_sample"])
        thin = MODULE.statistics([100, 200, 900])
        self.assertEqual((thin["p50_ms"], thin["p95_ms"]), (200, 830))
        self.assertTrue(thin["small_sample"])

    def test_stalls_are_a_rate_not_part_of_the_distribution(self):
        report = MODULE.analyze(
            [
                captured(turn=1),
                captured(turn=2, tool_batches=1, server_total_ms=4_730),
                captured(turn=3, server_total_ms=12_000, elapsed_ms=11_970),
            ]
        )
        self.assertEqual(report["stalls"], {"count": 1, "of": 3})
        self.assertEqual(report["beyond_window"]["count"], 2)
        self.assertEqual(sum(report["dominant"].values()), 2)
        self.assertEqual(
            report["server_total_by_origin"]["last_voiced_pcm"]["max_ms"], 4_730
        )
        self.assertEqual(report["beyond_window"]["max_ms"], 1_730)
        self.assertEqual(
            {
                kind: stats["count"]
                for kind, stats in report["beyond_window_by_kind"].items()
            },
            {"last_voiced_pcm_no_tool": 1, "last_voiced_pcm_tool": 1},
        )
        summary = MODULE.summary_lines(report)
        self.assertIn("stalls=1/3", summary)
        self.assertIn("kind_last_voiced_pcm_tool=n1 p50=1730 p95=1730 small_n", summary)
        # The stall is still the worst reply, so it stays findable.
        self.assertEqual(MODULE.turn_id(report["worst"][0]), "r/s1/t3")

    def test_tool_share_counts_tool_replies_per_origin(self):
        report = MODULE.analyze(
            [
                captured(turn=1),
                captured(turn=2, tool_batches=1, server_total_ms=4_730),
                captured(turn=3, origin="prompt", server_total_ms=800),
                # A stall is a rate, so it stays out of the share too.
                captured(turn=4, tool_batches=1, server_total_ms=12_000),
            ]
        )
        self.assertEqual(
            report["tool_share"],
            {
                "last_voiced_pcm": {"tool": 1, "of": 2},
                "prompt": {"tool": 0, "of": 1},
            },
        )
        self.assertIn("tool_share_last_voiced_pcm=1/2", MODULE.summary_lines(report))

    def test_unmeasured_replies_are_split_by_tool_round(self):
        report = MODULE.analyze(
            [
                line(
                    stage="native_audio_cancelled",
                    turn=1,
                    captured=False,
                    tool_batches=1,
                ),
                line(stage="turn_complete", turn=2, elapsed_ms=2_000),
            ]
        )
        self.assertEqual(report["unmeasured_by_tool"], {"tool": 1, "no_tool": 1})

    def test_an_acknowledgement_reports_how_much_sooner_something_was_heard(self):
        report = MODULE.analyze(
            [
                captured(
                    turn=1, tool_batches=1, first_tool_call_ms=3_150, tool_ack_ms=3_160
                ),
                captured(turn=2),
            ]
        )
        self.assertEqual(report["tool_ack_lead"]["count"], 1)
        # To the reply's capture, not its acceptance: queued behind the clip,
        # the reply is heard only when the clip has played.
        self.assertEqual(report["tool_ack_lead"]["p50_ms"], 570)
        self.assertIn(
            "tool_ack_lead=n1 p50=570 p95=570 small_n", MODULE.summary_lines(report)
        )

    def test_tool_rounds_after_the_first_audio_do_not_make_a_tool_reply(self):
        report = MODULE.analyze(
            [
                captured(turn=1, tool_batches=0),
                line(stage="turn_complete", turn=1, tool_batches=1, elapsed_ms=5_000),
            ]
        )
        self.assertEqual(report["turns"][0]["tool_batches"], 0)
        self.assertEqual(
            list(report["beyond_window_by_kind"]), ["last_voiced_pcm_no_tool"]
        )

    def test_a_sample_of_only_stalls_is_slow_not_missing(self):
        report = MODULE.analyze([captured(server_total_ms=12_000, elapsed_ms=11_970)])
        self.assertEqual(report["beyond_window"]["count"], 0)
        self.assertEqual(MODULE.verdict(report), "SLOW")
        # An origin with nothing but stalls has no total to print, not a crash.
        summary = MODULE.summary_lines(report)
        self.assertFalse(any(line.startswith("total_") for line in summary))
        self.assertIn("beyond_window_with_stalls=n1 p50=9000 p95=9000 small_n", summary)

    def test_an_end_arriving_with_the_first_output_is_counted(self):
        report = MODULE.analyze(
            [
                captured(
                    turn=1, provider_voice_end_ms=3_500, first_provider_output_ms=3_505
                ),
                captured(turn=2),
            ]
        )
        self.assertEqual(report["end_with_output"], {"count": 1, "of": 2})
        self.assertIn("end_with_output=1/2", MODULE.summary_lines(report))

    def test_records_join_across_sockets_and_keep_unmeasured_turns(self):
        report = MODULE.analyze(
            [
                line(stage="socket_replaced", socket=2, elapsed_ms=900),
                captured(socket=2),
                line(stage="turn_complete", elapsed_ms=4_000, socket=2),
                line(stage="native_audio_cancelled", turn=2, captured=False),
                line(stage="interrupted", turn=2, elapsed_ms=50),
                line(stage="turn_complete", turn=3, elapsed_ms=2_000),
            ]
        )
        first, cancelled, silent = report["turns"]
        self.assertEqual(
            [e["stage"] for e in first["timeline"]],
            ["socket_replaced", "native_audio_capture", "turn_complete"],
        )
        self.assertEqual(first["outcome"], "turn_complete")
        self.assertEqual(cancelled["outcome"], "interrupted")
        self.assertFalse(cancelled["captured"])
        self.assertEqual(
            report["unmeasured"], {"native_audio_cancelled": 1, "no_audio": 1}
        )
        self.assertEqual(
            report["server_total_by_origin"]["last_voiced_pcm"]["count"], 1
        )

    def test_replacement_of_an_unstarted_reply_keeps_the_outcome_open(self):
        report = MODULE.analyze([line(stage="socket_replaced", elapsed_ms=5_000)])
        self.assertEqual(report["turns"][0]["outcome"], "open")
        tool_wait = MODULE.analyze(
            [
                line(
                    stage="socket_replaced",
                    elapsed_ms=5_000,
                    first_provider_output_ms=1_000,
                    continuing=True,
                )
            ]
        )
        self.assertEqual(tool_wait["turns"][0]["outcome"], "open")

    def test_receipt_before_the_first_local_input_has_a_signed_session_time(self):
        report = MODULE.analyze(
            [
                line(
                    origin="provider_event",
                    stage="turn_complete",
                    elapsed_ms=5,
                    observed_ms=-5,
                ),
            ]
        )
        self.assertEqual(report["turns"][0]["outcome"], "turn_complete")
        self.assertEqual(report["turns"][0]["timeline"][0]["observed_ms"], -5)

    def test_moving_the_origin_does_not_move_a_reconnect_after_completion(self):
        report = MODULE.analyze(
            [
                line(stage="socket_replaced", elapsed_ms=5_000, observed_ms=5_000),
                captured(elapsed_ms=1_000, server_total_ms=1_030, observed_ms=7_030),
                line(stage="turn_complete", elapsed_ms=2_000, observed_ms=8_000),
            ]
        )
        row = report["turns"][0]
        self.assertEqual(row["outcome"], "turn_complete")
        self.assertEqual(
            [e["stage"] for e in row["timeline"]],
            ["socket_replaced", "native_audio_capture", "turn_complete"],
        )

    def test_an_enqueue_failure_is_counted_without_becoming_a_latency_sample(self):
        report = MODULE.analyze(
            [
                line(
                    stage="audio_enqueue_failed",
                    captured=False,
                    elapsed_ms=500,
                    server_total_ms=510,
                    enqueued=False,
                )
            ]
        )
        self.assertEqual(report["unmeasured"], {"audio_enqueue_failed": 1})
        self.assertEqual(report["beyond_window"]["count"], 0)
        self.assertEqual(MODULE.verdict(report), "NO_DATA")

    def test_percentiles_exclude_provider_event_origins(self):
        report = MODULE.analyze(
            [
                captured(turn=n, server_total_ms=3_030 + n, elapsed_ms=3_000 + n)
                for n in range(1, 21)
            ]
            + [captured(turn=21, origin="provider_event", server_total_ms=5)]
        )
        self.assertEqual(report["beyond_window"]["count"], 20)
        self.assertEqual(report["beyond_window"]["p95_ms"], 49)
        self.assertEqual(report["server_total_by_origin"]["provider_event"]["count"], 1)

    def test_slow_turns_are_judged_after_the_end_of_speech(self):
        report = MODULE.analyze(
            [captured(), captured(turn=2, elapsed_ms=5_200, server_total_ms=5_230)],
            slow_ms=1_500,
        )
        self.assertEqual(report["slow_turns"], ["r/s1/t2"])
        self.assertEqual(report["worst"][0]["turn"], 2)
        self.assertEqual(MODULE.verdict(report), "SLOW")

    def test_incomplete_and_unbounded_capture_records_do_not_crash_aggregation(self):
        report = MODULE.analyze(
            [
                captured(elapsed_ms=None),
                captured(server_total_ms=None),
                captured(tool_batches=[]),
                captured(silence_window_ms=-1),
                line(stage="native_audio_capture", captured="yes"),
                captured(server_total_ms=10**500),
            ]
        )
        self.assertEqual(report["rejected_records"], 6)
        self.assertEqual(report["turns"], [])

    def test_false_capture_stages_and_deeply_nested_json_are_rejected(self):
        report = MODULE.analyze(
            [
                line(
                    stage="native_audio_cancelled", captured=True, server_total_ms=500
                ),
                line(stage="audio_accepted", captured=True, server_total_ms=500),
                "codetrial latency " + "[" * 2_000 + "]" * 2_000,
            ]
        )
        self.assertEqual(report["rejected_records"], 3)
        self.assertEqual(report["turns"], [])

    def test_provider_only_captures_do_not_claim_healthy_input_latency(self):
        report = MODULE.analyze([captured(origin="provider_event")])
        self.assertTrue(report["turns"][0]["captured"])
        self.assertEqual(report["beyond_window"]["count"], 0)
        self.assertEqual(MODULE.verdict(report), "NO_DATA")

    def test_invalid_records_are_rejected_not_crashed_on(self):
        report = MODULE.analyze(
            [
                "codetrial latency {",
                line(version=1, stage="turn_complete"),
                line(stage="turn_complete", origin=["bad"]),
                line(stage="made_up"),
                line(stage="turn_complete", elapsed_ms=True),
                captured(server_total_ms=float("nan")),
                captured(server_total_ms=-4),
                line(stage="turn_complete", session="1"),
            ]
        )
        self.assertEqual(report["turns"], [])
        self.assertEqual(report["rejected_records"], 8)


class CliTests(unittest.TestCase):
    def test_no_records_fails(self):
        result = run("--summary", stdin="unrelated\n")
        self.assertEqual(result.returncode, 1)
        self.assertIn("verdict=NO_DATA", result.stdout)

    def test_a_truncated_line_fails_only_when_strict(self):
        stdin = "\n".join([captured(), 'codetrial latency {"event"']) + "\n"
        self.assertEqual(run("--summary", stdin=stdin).returncode, 0)
        self.assertEqual(run("--summary", "--strict", stdin=stdin).returncode, 1)

    def test_a_comparison_reads_the_earlier_log_from_a_file(self):
        with tempfile.TemporaryDirectory() as scratch:
            before = Path(scratch) / "before.log"
            before.write_text(
                "\n".join(
                    captured(turn=turn, tool_batches=1, server_total_ms=4_000)
                    for turn in (1, 2)
                )
                + "\n"
            )
            after = "\n".join(
                [
                    captured(turn=1, tool_batches=1, server_total_ms=4_000),
                    captured(turn=2, server_total_ms=3_200),
                ]
            )
            text = run("--compare", str(before), stdin=after + "\n")
            self.assertEqual(text.returncode, 0, text.stderr)
            self.assertIn("tool round first in 2/2 -> 1/2", text.stdout)
            self.assertIn("mix -400.0, speed +0.0", text.stdout)
            self.assertIn("stalls 0/2 -> 0/2", text.stdout)
            report = json.loads(
                run("--compare", str(before), "--json", stdin=after + "\n").stdout
            )
            self.assertEqual(
                report["origins"]["last_voiced_pcm"]["mean_ms"],
                {"before": 1_000.0, "after": 600.0},
            )
            # Strict mode holds both logs to what it holds one to.
            self.assertEqual(
                run(
                    "--compare", str(before), "--strict", stdin=after + "\n"
                ).returncode,
                0,
            )
            truncated = after + '\ncodetrial latency {"event"\n'
            self.assertEqual(
                run("--compare", str(before), stdin=truncated).returncode, 0
            )
            self.assertEqual(
                run("--compare", str(before), "--strict", stdin=truncated).returncode,
                1,
            )
            with open(before, "a", encoding="utf-8") as stream:
                stream.write('codetrial latency {"event"\n')
            self.assertEqual(
                run(
                    "--compare", str(before), "--strict", stdin=after + "\n"
                ).returncode,
                1,
            )

    def test_text_and_json_reports(self):
        text = run(stdin=captured() + "\n")
        self.assertEqual(text.returncode, 0, text.stderr)
        self.assertIn("silence_window", text.stdout)
        self.assertIn("verdict=HEALTHY", text.stdout)
        report = json.loads(run("--json", stdin=captured() + "\n").stdout)
        self.assertEqual(sum(report["turns"][0]["waterfall_ms"].values()), 3_730)


class ComparisonTests(unittest.TestCase):
    @staticmethod
    def report(*beyond):
        """Spoken replies, each (tool round first, ms beyond the window)."""
        return MODULE.analyze(
            [
                captured(
                    turn=turn,
                    tool_batches=int(tool),
                    server_total_ms=3_000 + ms,
                )
                for turn, (tool, ms) in enumerate(beyond, 1)
            ]
        )

    def test_the_change_in_the_mean_splits_exactly_into_mix_and_speed(self):
        before = self.report(*[(True, 1_000)] * 4)
        after = self.report(*[(True, 1_200)] * 2, *[(False, 200)] * 2)
        entry = MODULE.compare(before, after)["origins"]["last_voiced_pcm"]
        self.assertEqual(
            entry["tool_share"],
            {"before": {"tool": 4, "of": 4}, "after": {"tool": 2, "of": 4}},
        )
        self.assertEqual(entry["mean_ms"], {"before": 1_000.0, "after": 700.0})
        # Half the replies no longer wait on a tool round, which is worth
        # 450 ms, while the tool replies that remain each got 200 ms slower.
        self.assertEqual(entry["mix_ms"], -450.0)
        self.assertEqual(entry["speed_ms"], 150.0)
        self.assertEqual(
            entry["mix_ms"] + entry["speed_ms"],
            entry["mean_ms"]["after"] - entry["mean_ms"]["before"],
        )
        self.assertEqual(
            entry["kinds"]["tool"]["shift"], {"ms": 200, "ci95_ms": [200, 200]}
        )
        # A kind one side never saw has no interval to give.
        self.assertIsNone(entry["kinds"]["no_tool"]["shift"])
        self.assertEqual(entry["kinds"]["no_tool"]["before"], {"count": 0})

    def test_a_side_without_replies_has_no_mix_or_speed(self):
        entry = MODULE.compare(self.report(), self.report((True, 1_000)))["origins"][
            "last_voiced_pcm"
        ]
        self.assertEqual(entry["mean_ms"], {"before": None, "after": 1_000.0})
        self.assertIsNone(entry["mix_ms"])
        self.assertIsNone(entry["speed_ms"])

    def test_the_interval_is_seeded_and_spans_the_resampled_shift(self):
        before = self.report(*[(True, ms) for ms in (800, 900, 1_000, 1_100, 1_200)])
        after = self.report(*[(True, ms) for ms in (700, 850, 1_000, 1_150, 1_300)])
        first = MODULE.compare(before, after)
        self.assertEqual(first, MODULE.compare(before, after))
        shift = first["origins"]["last_voiced_pcm"]["kinds"]["tool"]["shift"]
        self.assertEqual(shift["ms"], 0)
        low, high = shift["ci95_ms"]
        self.assertLess(low, 0)
        self.assertGreater(high, 0)

    def test_stalls_are_compared_as_rates(self):
        before = self.report((True, 1_000), (True, 9_000))
        after = self.report((True, 1_000), (False, 200))
        comparison = MODULE.compare(before, after)
        self.assertEqual(
            comparison["stalls"],
            {"before": {"count": 1, "of": 2}, "after": {"count": 0, "of": 2}},
        )
        self.assertEqual(
            comparison["origins"]["last_voiced_pcm"]["tool_share"]["before"],
            {"tool": 1, "of": 1},
        )


def owed(stage="audio_accepted", outcome="open", **fields):
    outcome = fields.pop("obligation_outcome", outcome)
    return line(
        stage=stage,
        origin="prompt",
        elapsed_ms=fields.pop("elapsed_ms", 900),
        observed_ms=fields.pop("observed_ms", 900),
        obligation=fields.pop("obligation", 1),
        obligation_outcome=outcome,
        obligation_episodes=fields.pop("obligation_episodes", []),
        obligation_episode_count=fields.pop("obligation_episode_count", 0),
        interview_ms=fields.pop("interview_ms", 61_000),
        owed_start_interview_ms=fields.pop("owed_start_interview_ms", 60_100),
        first_accept_ms=fields.pop("first_accept_ms", 800),
        first_accept_socket=fields.pop("first_accept_socket", 2),
        **fields,
    )


class ObligationTests(unittest.TestCase):
    def summary(self, lines):
        report = MODULE.analyze(lines)
        return report, report["turns"][0]["obligation"]

    def test_a_reply_that_crossed_a_replacement_is_one_answered_obligation(self):
        report, obligation = self.summary(
            [
                owed(
                    "socket_replaced",
                    obligation_episodes=[1],
                    obligation_episode_count=1,
                    continuing=True,
                    elapsed_ms=300,
                    observed_ms=300,
                ),
                owed(obligation_episodes=[1, 2], obligation_episode_count=2),
                captured(
                    origin="prompt",
                    obligation=1,
                    obligation_outcome="open",
                    obligation_episodes=[1, 2],
                    obligation_episode_count=2,
                    observed_ms=950,
                ),
                owed(
                    "turn_complete",
                    "answered",
                    obligation_episodes=[1, 2],
                    obligation_episode_count=2,
                    observed_ms=1_000,
                ),
            ]
        )
        self.assertEqual(obligation["id"], 1)
        self.assertEqual(obligation["origin"], "prompt")
        self.assertEqual(obligation["episodes"], [1, 2])
        self.assertEqual(obligation["episode_count"], 2)
        self.assertEqual(obligation["owed_start_interview_ms"], 60_100)
        self.assertEqual(obligation["first_accept_socket"], 2)
        self.assertTrue(obligation["native_capture"])
        self.assertEqual(obligation["outcome"], "answered")
        self.assertEqual(
            report["obligations"],
            {
                "count": 1,
                "outcomes": {"answered": 1},
                "carried": 1,
                "carried_unanswered": 0,
            },
        )

    def test_superseded_and_ended_replies_are_not_answers(self):
        report = MODULE.analyze(
            [
                owed("held", "superseded", obligation=1, turn=1),
                owed("turn_complete", "ended", obligation=2, turn=2),
            ]
        )
        self.assertEqual(
            report["obligations"]["outcomes"], {"superseded": 1, "ended": 1}
        )

    def test_a_reply_with_no_terminal_record_is_missing_not_answered(self):
        report, obligation = self.summary(
            [owed(obligation_episodes=[1], obligation_episode_count=1)]
        )
        self.assertEqual(obligation["outcome"], "missing")
        self.assertEqual(report["obligations"]["outcomes"], {"missing": 1})
        self.assertEqual(report["obligations"]["carried_unanswered"], 1)

    def test_duplicate_records_do_not_duplicate_an_obligation(self):
        record = owed("turn_complete", "answered")
        report = MODULE.analyze([record, record])
        self.assertEqual(report["obligations"]["count"], 1)

    def test_legacy_records_have_no_obligation_and_an_unchanged_summary(self):
        report = MODULE.analyze([captured()])
        self.assertIsNone(report["turns"][0]["obligation"])
        self.assertEqual(report["obligations"]["count"], 0)
        self.assertFalse(
            any(text.startswith("obligation") for text in MODULE.summary_lines(report))
        )

    def test_the_summary_reports_obligation_outcomes(self):
        report = MODULE.analyze(
            [owed("turn_complete", "answered", obligation_episode_count=1)]
        )
        text = MODULE.summary_lines(report)
        self.assertIn("obligations=1", text)
        self.assertIn("obligation_answered=1", text)
        self.assertIn("obligations_carried=1", text)
        self.assertIn("obligations_carried_unanswered=0", text)

    def test_a_non_owed_reply_carries_null_obligation_fields(self):
        report = MODULE.analyze(
            [
                line(
                    stage="audio_accepted",
                    origin="provider_event",
                    obligation=None,
                    obligation_outcome=None,
                    obligation_episodes=[],
                    obligation_episode_count=0,
                    interview_ms=5,
                )
            ]
        )
        self.assertEqual(report["rejected_records"], 0)
        self.assertIsNone(report["turns"][0]["obligation"])

    def test_malformed_obligation_fields_are_rejected(self):
        for field, value in (
            ("obligation", "1"),
            ("obligation", -1),
            ("obligation_outcome", "answered-ish"),
            ("obligation_episodes", [0]),
            ("obligation_episodes", list(range(1, 10))),
            ("obligation_episodes", "1"),
            ("obligation_episode_count", -1),
            ("interview_ms", 1.5),
            ("first_accept_socket", True),
        ):
            report = MODULE.analyze([owed(**{field: value})])
            self.assertEqual(report["rejected_records"], 1, (field, value))
        self.assertEqual(
            MODULE.analyze([line(stage="audio_accepted", obligation=1)])[
                "rejected_records"
            ],
            1,
            "an obligation needs an outcome",
        )


def pressure(**fields):
    return "Oct 14 agent[12]: codetrial latency " + json.dumps(
        {
            "event": "socket_pressure",
            "version": 2,
            "pid": 7,
            "room": "r",
            "session": 1,
            "socket": 1,
            "end": "replacement",
            "reader_end": "stopped",
            "interview_ms": 61_000,
            "capacity": 256,
            "occupancy_high_water": 256,
            "blocked_sends": 1,
            "blocked_ms_total": 60_000,
            "blocked_ms_max": 60_000,
            "received": 257,
            "consumed": 0,
            **fields,
        }
    )


class SocketPressureTests(unittest.TestCase):
    def test_a_summary_is_reported_with_its_exact_counters(self):
        report = MODULE.analyze([pressure()])
        summary = report["socket_pressure"]
        self.assertEqual(report["rejected_records"], 0)
        self.assertEqual(summary["sockets"], 1)
        self.assertEqual(summary["blocked_sockets"], 1)
        self.assertEqual(summary["blocked_sends"], 1)
        self.assertEqual(summary["blocked_ms_total"], 60_000)
        self.assertEqual(summary["blocked_ms_max"], 60_000)
        self.assertEqual(summary["high_water_max"], 256)
        self.assertEqual(summary["full_sockets"], 1)
        self.assertEqual(summary["partial"], 0)
        row = summary["rows"][0]
        self.assertEqual((row["received"], row["consumed"]), (257, 0))
        self.assertEqual(row["reader_end"], "stopped")

    def test_pressure_records_are_not_turns_and_do_not_disturb_them(self):
        base = MODULE.analyze([captured()])
        mixed = MODULE.analyze([pressure(), captured(), pressure(socket=2)])
        self.assertEqual(mixed["turns"], base["turns"])
        self.assertEqual(mixed["rejected_records"], 0)
        self.assertEqual(mixed["socket_pressure"]["sockets"], 2)
        self.assertEqual(base["socket_pressure"]["sockets"], 0)

    def test_read_idle_is_not_blocked_time(self):
        report = MODULE.analyze(
            [
                pressure(
                    reader_end="read_idle",
                    occupancy_high_water=0,
                    blocked_sends=0,
                    blocked_ms_total=0,
                    blocked_ms_max=0,
                    received=0,
                )
            ]
        )["socket_pressure"]
        self.assertEqual(report["read_idle_ends"], 1)
        self.assertEqual(report["blocked_sockets"], 0)
        self.assertEqual(report["blocked_ms_total"], 0)
        self.assertEqual(report["full_sockets"], 0)

    def test_duplicate_copies_count_one_socket(self):
        report = MODULE.analyze([pressure(), pressure(), pressure()])
        summary = report["socket_pressure"]
        self.assertEqual(summary["records"], 3)
        self.assertEqual(summary["sockets"], 1)
        self.assertEqual(summary["duplicates"], 2)
        self.assertEqual(summary["conflicts"], 0)
        self.assertEqual(summary["blocked_sends"], 1)
        self.assertEqual(summary["blocked_ms_total"], 60_000)

    def test_a_join_of_two_processes_keeps_their_sockets_apart(self):
        report = MODULE.analyze([pressure(), pressure(pid=8)])["socket_pressure"]
        self.assertEqual(report["sockets"], 2)
        self.assertEqual(report["blocked_ms_total"], 120_000)

    def test_partial_records_leave_unknown_fields_unknown(self):
        partial = pressure()
        fields = json.loads(partial.split("codetrial latency ", 1)[1])
        for name in ("blocked_sends", "blocked_ms_total", "blocked_ms_max"):
            del fields[name]
        text = "x: codetrial latency " + json.dumps(fields)
        report = MODULE.analyze([text])["socket_pressure"]
        self.assertEqual(report["sockets"], 1)
        self.assertEqual(report["partial"], 1)
        self.assertEqual(report["blocked_unknown"], 1)
        self.assertEqual(report["blocked_sockets"], 0)
        self.assertIsNone(report["blocked_sends"])
        self.assertIsNone(report["blocked_ms_total"])
        self.assertIsNone(report["blocked_ms_max"])
        self.assertIsNone(report["rows"][0]["blocked_sends"])
        lines = MODULE.summary_lines(MODULE.analyze([text]))
        self.assertIn("socket_pressure_blocked_unknown=1", lines)
        self.assertFalse(
            [name for name in lines if name.startswith("socket_pressure_blocked_ms")],
            "an unreported total is left out, not printed as 0",
        )

    def test_a_partial_copy_is_completed_by_a_full_one_without_zeroing(self):
        fields = json.loads(pressure().split("codetrial latency ", 1)[1])
        del fields["blocked_ms_total"]
        del fields["blocked_ms_max"]
        partial = "x: codetrial latency " + json.dumps(fields)
        summary = MODULE.analyze([partial, pressure()])["socket_pressure"]
        self.assertEqual(summary["sockets"], 1)
        self.assertEqual(summary["partial"], 0)
        self.assertEqual(summary["blocked_ms_total"], 60_000)
        self.assertEqual(summary["conflicts"], 0)
        # Nothing known from the copy that has it is lost to the one that does
        # not, in either order.
        summary = MODULE.analyze([pressure(), partial])["socket_pressure"]
        self.assertEqual(summary["blocked_ms_total"], 60_000)

    def test_disagreeing_copies_are_counted_and_the_first_is_kept(self):
        summary = MODULE.analyze(
            [pressure(), pressure(blocked_ms_total=70_000, blocked_ms_max=70_000)]
        )["socket_pressure"]
        self.assertEqual(summary["sockets"], 1)
        self.assertEqual(summary["conflicts"], 1, "one copy, however many fields")
        self.assertEqual(summary["blocked_ms_total"], 60_000)

    def test_copies_that_only_together_are_impossible_are_not_merged(self):
        def without(**gone):
            fields = json.loads(pressure().split("codetrial latency ", 1)[1])
            for name in gone["names"]:
                del fields[name]
            fields.update(gone.get("set", {}))
            return "x: codetrial latency " + json.dumps(fields)

        # Each copy is possible alone: consumed 10 with no received, received 3
        # with no consumed. Together they are not, so the second adds nothing.
        first = without(names=["received"], set={"consumed": 10})
        second = without(names=["consumed"], set={"received": 3})
        summary = MODULE.analyze([first, second])["socket_pressure"]
        self.assertEqual(summary["conflicts"], 1)
        row = summary["rows"][0]
        self.assertEqual(row["consumed"], 10)
        self.assertIsNone(row["received"])

    def test_a_null_field_is_unknown_not_zero(self):
        summary = MODULE.analyze(
            [pressure(blocked_ms_total=None, blocked_ms_max=None)]
        )["socket_pressure"]
        self.assertIsNone(summary["blocked_ms_total"])
        self.assertEqual(summary["partial"], 1)

    def test_impossible_or_malformed_summaries_are_rejected(self):
        for fields in (
            {"capacity": -1},
            {"capacity": 1.5},
            {"capacity": True},
            {"blocked_sends": "1"},
            {"occupancy_high_water": 257},
            {"consumed": 258},
            {"blocked_ms_max": 60_001},
            {"blocked_sends": 0},
            {"end": "later"},
            {"reader_end": 3},
            {"pid": None},
            {"room": 5},
            {"version": 1},
            {"interview_ms": 1.5},
        ):
            report = MODULE.analyze([pressure(**fields)])
            self.assertEqual(report["rejected_records"], 1, fields)
            self.assertEqual(report["socket_pressure"]["sockets"], 0, fields)

    def test_the_summary_lines_report_pressure_only_when_logged(self):
        quiet = "\n".join(MODULE.summary_lines(MODULE.analyze([captured()])))
        self.assertNotIn("socket_pressure", quiet)
        text = "\n".join(MODULE.summary_lines(MODULE.analyze([pressure()])))
        for expected in (
            "socket_pressure=1",
            "socket_pressure_blocked_sockets=1",
            "socket_pressure_full_sockets=1",
            "socket_pressure_blocked_sends=1",
            "socket_pressure_blocked_ms_total=60000",
            "socket_pressure_blocked_ms_max=60000",
            "socket_pressure_high_water_max=256",
            "socket_pressure_read_idle_ends=0",
        ):
            self.assertIn(expected, text)

    def test_the_cli_reports_pressure_from_a_mixed_log(self):
        result = run("--json", stdin="\n".join([captured(), pressure(), pressure()]))
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["socket_pressure"]["duplicates"], 1)
        text = run("--summary", stdin="\n".join([captured(), pressure()]))
        self.assertIn("socket_pressure_blocked_ms_total=60000", text.stdout)


# Owed gaps, episodes, exposure and the control adapter.
ONE_HOUR_MS = 3_600_000
CONTROL_SHA = "1ceda0efffe4e934d63c15867788b38258b92c95"


def reply(
    turn,
    start,
    end,
    *,
    episodes=(),
    accept_socket=2,
    socket=2,
    terminal="answered",
    capture=True,
    **scope,
):
    """The records of one owed reply: accepted, natively captured at `end` on
    the assessment clock (when it was), and ended."""
    shared = dict(
        turn=turn,
        obligation=turn,
        owed_start_interview_ms=start,
        first_accept_ms=None if accept_socket is None else 800,
        first_accept_socket=accept_socket,
        socket=socket,
        obligation_episodes=list(episodes),
        obligation_episode_count=len(episodes),
        **scope,
    )
    lines = []
    if accept_socket is not None:
        lines.append(owed("audio_accepted", interview_ms=start + 800, **shared))
    if capture:
        lines.append(
            captured(
                origin="prompt",
                obligation_outcome="open",
                interview_ms=end,
                observed_ms=950,
                **shared,
            )
        )
    if terminal:
        stage = {
            "answered": "turn_complete",
            "superseded": "held",
            "ended": "turn_complete",
        }
        lines.append(
            owed(
                stage[terminal],
                terminal,
                interview_ms=end if end is not None else start + 1_000,
                observed_ms=1_000,
                **shared,
            )
        )
    return lines


def clock(sample, elapsed, *, pid=7, session=1, room="r", start=1_700_000_000_000):
    return (
        f"interview clock: pid={pid} session={session} start_unix_ms={start} "
        f"elapsed_ms={elapsed} sample={sample} room={room}"
    )


def hour_clock(**scope):
    return [clock(0, 0, **scope), clock(60, ONE_HOUR_MS, **scope)]


def episode_start(episode, old, obligation, at, room="r"):
    return (
        f"replacement start: pid=7 session=1 episode={episode} trigger=goaway "
        f"old_socket={old} obligation={obligation} obligation_origin=prompt "
        f"advisories=1 interview_ms={at} room={room}"
    )


def episode_end(
    episode, old, new, obligation, *, result="installed", install=500, at=None
):
    return (
        f"replacement end: pid=7 session=1 episode={episode} result={result} "
        f"trigger=goaway old_socket={old} new_socket={new} replacement={new} "
        f"attempt=1 resume_tried=true obligation={obligation} notice_ms=10 "
        f"setup_ms=200 connected_ms=300 opener_ms=350 install_ms={install} "
        f"interview_ms={at} room=r"
    )


class OwedGapTests(unittest.TestCase):
    def test_the_gap_runs_from_the_original_start_to_the_first_native_capture(self):
        report = MODULE.analyze(reply(1, 60_100, 63_000, episodes=[1, 2]))
        gap = report["turns"][0]["owed"]
        self.assertEqual(gap["state"], "captured")
        self.assertEqual(gap["owed_gap_s"], 2.9)
        self.assertFalse(gap["censored"])
        self.assertEqual(gap["end_source"], "capture_record")
        self.assertEqual(report["owed"]["gap_s_total"], 2.9)

    def test_a_failed_capture_does_not_end_the_debt(self):
        lines = reply(1, 60_100, 63_000, capture=False) + [
            line(
                stage="audio_enqueue_failed",
                captured=False,
                turn=1,
                obligation=1,
                obligation_outcome="open",
                owed_start_interview_ms=60_100,
                interview_ms=61_000,
                observed_ms=920,
                origin="prompt",
            ),
            clock(0, 0),
            clock(2, 100_000),
        ]
        gap = MODULE.analyze(lines)["turns"][0]["owed"]
        self.assertEqual(gap["state"], "uncaptured")
        self.assertTrue(gap["censored"])
        self.assertEqual(gap["end_source"], "run_end")
        self.assertEqual(gap["owed_gap_ms"], 39_900)

    def test_an_unresolved_reply_is_censored_at_the_observed_run_end(self):
        lines = reply(1, 60_100, None, capture=False, terminal=None) + [
            clock(0, 0),
            clock(2, 100_000),
        ]
        report = MODULE.analyze(lines)
        gap = report["turns"][0]["owed"]
        self.assertEqual(gap["state"], "missing")
        self.assertTrue(gap["censored"])
        self.assertEqual(gap["end_interview_ms"], 100_000)
        self.assertEqual(report["owed"]["censored"], 1)
        self.assertEqual(report["owed"]["known"], 1)

    def test_censored_replies_stay_in_the_denominators_and_every_state_is_kept(self):
        lines = (
            reply(1, 10_000, 12_000)
            + reply(2, 20_000, None, capture=False, terminal=None)
            + reply(
                3,
                30_000,
                31_000,
                capture=False,
                terminal="superseded",
                accept_socket=None,
            )
            + reply(
                4, 40_000, 41_000, capture=False, terminal="ended", accept_socket=None
            )
            + reply(5, 50_000, 51_000, capture=False)
            + [
                line(
                    stage="turn_complete",
                    turn=6,
                    origin="provider_event",
                    interview_ms=70_000,
                )
            ]
            + [captured(turn=7)]
            + hour_clock()
        )
        owed = MODULE.analyze(lines)["owed"]
        self.assertEqual(
            owed["states"],
            {"captured": 1, "missing": 1, "superseded": 1, "ended": 1, "uncaptured": 1},
        )
        self.assertEqual(owed["count"], 5)
        self.assertEqual(owed["censored"], 2)
        self.assertEqual(owed["known"], 5)
        self.assertEqual(owed["no_reply_owed"], 1)
        self.assertEqual(owed["missing_identity"], 1)
        self.assertEqual(sum(owed["states"].values()), owed["count"])

    def test_unknown_endpoints_are_retained_with_no_gap(self):
        lines = [
            owed(
                "audio_accepted",
                turn=1,
                owed_start_interview_ms=None,
                interview_ms=None,
            ),
            owed(
                "audio_accepted",
                turn=1,
                room="x",
                owed_start_interview_ms=5_000,
                interview_ms=None,
            ),
        ]
        owed_report = MODULE.analyze(lines)["owed"]
        self.assertEqual(owed_report["count"], 2)
        self.assertEqual(owed_report["unknown"], 2)
        self.assertEqual(
            owed_report["unknown_reasons"], {"unknown_start": 1, "unknown_end": 1}
        )
        self.assertEqual(owed_report["known"], 0)
        self.assertIsNone(owed_report["gap_s_total"])
        self.assertIsNone(owed_report["owed_s_per_interview_hour"])

    def test_a_capture_without_a_clock_reading_uses_the_origin_offset(self):
        lines = reply(1, 60_000, 0)
        lines = [
            text.replace('"interview_ms": 0', '"interview_ms": null')
            if '"native_audio_capture"' in text
            else text
            for text in lines
        ]
        gap = MODULE.analyze(lines)["turns"][0]["owed"]
        self.assertEqual(gap["end_source"], "origin_offset")
        self.assertEqual(gap["owed_gap_ms"], 3_730)

    def test_a_clock_that_runs_backwards_is_unknown_not_negative(self):
        gap = MODULE.analyze(reply(1, 90_000, 80_000))["turns"][0]["owed"]
        self.assertEqual(gap["unknown"], "clock_backwards")
        self.assertIsNone(gap["owed_gap_ms"])


class OwedExposureTests(unittest.TestCase):
    def rate(self, lines):
        return MODULE.analyze(lines)["owed"]

    def test_overlapping_and_carried_debt_is_counted_once_per_interview_hour(self):
        lines = (
            reply(1, 10_000, 20_000, episodes=[1, 2])
            + reply(2, 15_000, 30_000)
            + reply(3, 40_000, 50_000, episodes=[3])
            + hour_clock()
        )
        owed = self.rate(lines)
        scope = owed["scopes"][0]
        self.assertEqual(scope["summed_gap_ms"], 35_000)
        self.assertEqual(scope["owed_interval_ms"], 30_000)
        self.assertEqual(owed["exposure_s"], 3_600.0)
        self.assertEqual(owed["owed_s_per_interview_hour"], 30.0)

    def test_an_obligation_carried_through_episodes_is_one_interval(self):
        carried = reply(1, 10_000, 40_000, episodes=[1, 2, 3, 4])
        owed = self.rate(carried + hour_clock())
        self.assertEqual(owed["scopes"][0]["owed_interval_ms"], 30_000)
        self.assertEqual(owed["owed_s_per_interview_hour"], 30.0)

    def test_two_rooms_have_their_own_exposure_and_obligations(self):
        lines = (
            reply(1, 10_000, 20_000, room="a")
            + reply(1, 10_000, 20_000, room="b")
            + hour_clock(room="a")
            + hour_clock(room="b")
        )
        owed = self.rate(lines)
        self.assertEqual(owed["count"], 2)
        self.assertEqual(len(owed["scopes"]), 2)
        self.assertEqual(owed["exposure_s"], 7_200.0)
        self.assertEqual(owed["owed_s_per_interview_hour"], 10.0)

    def test_a_room_without_a_verified_clock_stays_out_of_the_rate_not_the_count(self):
        lines = (
            reply(1, 10_000, 20_000, room="a")
            + reply(1, 10_000, 20_000, room="b")
            + hour_clock(room="a")
        )
        owed = self.rate(lines)
        self.assertEqual(owed["count"], 2)
        self.assertEqual(owed["unverified_scopes"], 1)
        self.assertEqual(owed["exposure_s"], 3_600.0)
        self.assertEqual(owed["owed_s_per_interview_hour"], 10.0)
        reasons = {s["room"]: s["exposure_unverified"] for s in owed["scopes"]}
        self.assertEqual(reasons, {"a": None, "b": "no_clock_samples"})

    def test_pid_reuse_with_another_session_is_another_scope(self):
        lines = (
            reply(1, 10_000, 20_000, session=1)
            + reply(1, 10_000, 20_000, session=2)
            + hour_clock(session=1)
            + hour_clock(session=2, start=1_700_000_999_000)
        )
        report = MODULE.analyze(lines)
        self.assertEqual(len(report["turns"]), 2)
        self.assertEqual(report["owed"]["count"], 2)
        self.assertEqual(len(report["owed"]["scopes"]), 2)
        self.assertEqual(report["owed"]["owed_s_per_interview_hour"], 10.0)

    def test_unverifiable_clocks_give_no_exposure(self):
        replies = reply(1, 10_000, 20_000)
        cases = {
            "no_start_sample": [clock(1, 60_000)],
            "clock_went_backwards": [clock(0, 0), clock(1, 60_000), clock(2, 30_000)],
            "conflicting_anchor": [
                clock(0, 0),
                clock(1, 60_000, start=1_700_000_005_000),
            ],
            "conflicting_samples": [clock(0, 0), clock(1, 60_000), clock(1, 61_000)],
        }
        for reason, samples in cases.items():
            with self.subTest(reason):
                owed = self.rate(replies + samples)
                self.assertEqual(owed["scopes"][0]["exposure_unverified"], reason)
                self.assertIsNone(owed["owed_s_per_interview_hour"])
                self.assertEqual(owed["count"], 1)
                self.assertEqual(owed["unverified_scopes"], 1)

    def test_a_copied_log_changes_nothing_but_the_duplicate_count(self):
        lines = (
            reply(1, 10_000, 20_000, episodes=[1])
            + hour_clock()
            + [episode_start(1, 1, 1, 10_500), episode_end(1, 1, 2, 1, at=11_000)]
            + [pressure()]
        )
        once = MODULE.analyze(lines)
        twice = MODULE.analyze(lines + lines)
        self.assertEqual(once["owed"], twice["owed"])
        self.assertEqual(
            once["socket_pressure"]["sockets"], twice["socket_pressure"]["sockets"]
        )
        self.assertEqual(twice["duplicate_records"], 3)
        self.assertEqual(twice["lifecycle_conflicts"], 0)

    def test_line_order_does_not_change_the_result(self):
        lines = (
            reply(1, 10_000, 20_000, episodes=[1])
            + reply(2, 15_000, 30_000)
            + hour_clock()
            + [episode_start(1, 1, 1, 10_500), episode_end(1, 1, 2, 1, at=11_000)]
        )
        forward = MODULE.analyze(lines)
        backward = MODULE.analyze(list(reversed(lines)))
        self.assertEqual(forward["owed"], backward["owed"])

    def test_noise_and_partial_lines_are_rejected_and_do_not_move_the_result(self):
        lines = reply(1, 10_000, 20_000) + hour_clock()
        noisy = [
            "kernel: unrelated",
            'codetrial latency {"event": "reply_la',
            "interview clock: pid=7 session=1 start_unix_ms=none elapsed_ms= sample=3 room=r",
            "replacement start: pid=7 session=1 episode=0 trigger=x old_socket=1 "
            "obligation=none obligation_origin=none advisories=1 interview_ms=1 room=r",
            "replacement end: pid=7 session=1 episode=1 result=maybe trigger=x old_socket=1 "
            "new_socket=none replacement=none attempt=1 resume_tried=true obligation=none "
            "notice_ms=none setup_ms=none connected_ms=none opener_ms=none install_ms=none "
            "interview_ms=none room=r",
        ]
        clean = MODULE.analyze(lines)
        report = MODULE.analyze(noisy[:2] + lines + noisy[2:])
        self.assertEqual(report["rejected_records"], 4)
        self.assertEqual(report["owed"], clean["owed"])

    def test_a_cut_final_lifecycle_line_is_not_trusted(self):
        lines = reply(1, 10_000, 20_000, episodes=[1]) + [
            episode_start(1, 1, 1, 10_500)
        ]
        # Cut inside the room name, so it still has every field.
        end = episode_end(1, 1, 2, 1, at=11_000).replace("room=r", "room=")
        whole = episode_end(1, 1, 2, 1, at=11_000)
        trusted = MODULE.analyze(lines + [whole])
        self.assertEqual(trusted["owed"]["episodes"]["classes"], {"satisfied": 1})
        cut = MODULE.analyze(lines + [whole], tail_open=True)
        self.assertTrue(cut["truncated_final"])
        self.assertEqual(cut["owed"]["episodes"]["classes"], {"incomplete": 1})
        broken = MODULE.analyze(lines + [end], tail_open=True)
        self.assertTrue(broken["truncated_final"])
        self.assertEqual(broken["owed"]["episodes"]["classes"], {"incomplete": 1})

    def test_strict_fails_a_log_whose_final_lifecycle_line_was_dropped_as_cut(self):
        body = "\n".join(
            reply(1, 10_000, 20_000, episodes=[1])
            + hour_clock()
            + [episode_start(1, 1, 1, 10_500), episode_end(1, 1, 2, 1, at=11_000)]
        )
        for text, code in ((body + "\n", 0), (body, 1)):
            with self.subTest(newline=code == 0):
                result = run("--summary", "--strict", stdin=text)
                self.assertEqual(result.returncode, code, result.stdout)

    def test_a_billing_exit_with_no_ingress_line_keeps_the_room_counts_exact(self):
        lines = reply(1, 10_000, 20_000) + [
            episode_start(1, 1, 1, 10_500),
            episode_end(1, 1, "none", 1, result="no_setup", install="none", at=11_000),
        ]
        resources = MODULE.analyze(lines)["resources"]
        self.assertFalse(resources["ingress_lines_exceed_episode_ends"])
        self.assertTrue(resources["room_counts_exact"])

    def test_a_tail_cut_inside_the_marker_is_flagged(self):
        lines = reply(1, 10_000, 20_000)
        for tail in (
            "Oct 14 agent[12]: codetrial latenc",
            "interview clo",
            "replacement en",
        ):
            with self.subTest(tail=tail):
                report = MODULE.analyze(lines + [tail], tail_open=True)
                self.assertTrue(report["truncated_final"])
        self.assertFalse(
            MODULE.analyze(lines + ["unrelated"], tail_open=True)["truncated_final"]
        )

    def test_an_unrelated_cut_line_is_not_a_truncated_record(self):
        for tail in ("exit code", "exit status 1 in inte"):
            report = MODULE.analyze(reply(1, 1, 2) + [tail], tail_open=True)
            self.assertFalse(report["truncated_final"], tail)

    def test_an_end_from_the_origin_offset_extends_the_exposure(self):
        lines = [
            text.replace('"interview_ms": 0', '"interview_ms": null')
            if '"native_audio_capture"' in text
            else text
            for text in reply(1, 1_000, 0)
        ]
        owed = MODULE.analyze(lines + [clock(0, 0), clock(1, 2_000)])["owed"]
        self.assertEqual(owed["scopes"][0]["exposure_ms"], 1_000 + 3_730)

    def test_a_reply_whose_first_capture_failed_is_reported_apart(self):
        lines = [
            line(
                turn=1,
                origin="prompt",
                captured=False,
                stage="audio_enqueue_failed",
                obligation=1,
                obligation_outcome="open",
                owed_start_interview_ms=1_000,
                interview_ms=1_100,
                observed_ms=100,
            ),
            captured(
                turn=1,
                origin="prompt",
                obligation=1,
                obligation_outcome="open",
                owed_start_interview_ms=1_000,
                interview_ms=2_000,
                observed_ms=900,
            ),
        ]
        report = MODULE.analyze(lines)
        self.assertEqual(report["owed"]["states"], {"captured": 1})
        self.assertEqual(report["owed"]["captured_after_failed_first"], 1)
        self.assertEqual(report["turns"][0]["captured"], False)

    def test_a_clock_copy_with_another_anchor_removes_the_exposure(self):
        copy = clock(1, 60_000, start=1_700_000_005_000)
        owed = MODULE.analyze(reply(1, 1, 2) + [clock(0, 0), clock(1, 60_000), copy])[
            "owed"
        ]
        self.assertEqual(owed["scopes"][0]["exposure_unverified"], "conflicting_anchor")
        self.assertIsNone(owed["owed_s_per_interview_hour"])

    def test_offsets_run_from_the_origin_a_later_input_moved(self):
        # The earliest start is 1_000; the record that holds the output carries
        # the moved origin 3_000, and output came 500 ms after it.
        moved = reply(1, 3_000, 4_000, episodes=[1], accept_socket=2)
        moved = [
            text.replace(
                '"first_provider_output_ms": 3500', '"first_provider_output_ms": 500'
            )
            for text in moved
        ]
        early = owed(
            "audio_accepted",
            obligation=1,
            turn=1,
            owed_start_interview_ms=1_000,
            interview_ms=1_100,
            observed_ms=100,
            first_accept_socket=None,
            first_accept_ms=None,
        )
        lines = [
            early,
            *moved,
            episode_start(1, 1, 1, 1_500),
            episode_end(1, 1, 2, 1, at=2_000, install=700),
        ]
        report = MODULE.analyze(lines)
        row = report["owed"]["episodes"]["rows"][0]
        self.assertEqual(row["class"], "satisfied")
        self.assertEqual(report["turns"][0]["owed"]["start_interview_ms"], 1_000)
        # Installed at 1_500 + 700; output at 3_000 + 500.
        self.assertEqual(row["setup_to_first_output_ms"], 1_300)

    def test_the_cli_treats_a_final_line_without_a_newline_as_possibly_cut(self):
        body = "\n".join(
            reply(1, 10_000, 20_000, episodes=[1])
            + [episode_start(1, 1, 1, 10_500), episode_end(1, 1, 2, 1, at=11_000)]
        )
        for text, cut in ((body + "\n", False), (body, True)):
            with self.subTest(cut=cut), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "agent.log"
                path.write_text(text, encoding="utf-8")
                result = run("--json", str(path), stdin="")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["truncated_final"], cut)


class EpisodeTests(unittest.TestCase):
    def test_an_episode_is_satisfied_only_by_audio_on_a_newer_socket(self):
        lines = reply(1, 60_100, 63_000, episodes=[1], accept_socket=2) + [
            episode_start(1, 1, 1, 60_500),
            episode_end(1, 1, 2, 1, at=61_000),
        ]
        report = MODULE.analyze(lines)
        self.assertEqual(report["owed"]["episodes"]["classes"], {"satisfied": 1})

    def test_audio_from_a_prior_socket_never_satisfies_a_new_episode(self):
        # Accepted on socket 1, then socket 1 was replaced for an obligation the
        # log says was still owed: the prior socket's audio is not the answer.
        lines = (
            reply(1, 60_100, 63_000, episodes=[1], accept_socket=1, socket=1)
            + [episode_start(1, 1, 1, 60_500), episode_end(1, 1, 2, 1, at=61_000)]
            + reply(2, 70_000, 72_000, episodes=[2], accept_socket=2, socket=2)
            + [episode_start(2, 2, 2, 70_500), episode_end(2, 2, 3, 2, at=71_000)]
        )
        classes = {
            e["episode"]: e["class"]
            for e in MODULE.analyze(lines)["owed"]["episodes"]["rows"]
        }
        self.assertEqual(classes, {1: "prior_socket", 2: "prior_socket"})

    def test_every_episode_outcome_is_kept(self):
        lines = (
            reply(1, 60_000, 63_000, episodes=[1], accept_socket=2)
            + reply(2, 70_000, 73_000, episodes=[2], accept_socket=1, socket=1)
            + reply(3, 80_000, 83_000, episodes=[3], accept_socket=3, socket=3)
            + reply(7, 90_000, None, accept_socket=2, capture=False)
            + reply(
                8,
                95_000,
                95_500,
                accept_socket=None,
                capture=False,
                terminal="superseded",
            )
            # Audio from the replaced socket, in an episode that never installed.
            + reply(10, 97_000, 97_200, accept_socket=1, socket=1)
            # Audio from a socket newer than the replaced one but older than the
            # one installed.
            + reply(11, 98_000, 98_200, accept_socket=2, socket=2)
            + [
                episode_start(1, 1, 1, 60_500),
                episode_end(1, 1, 2, 1, at=61_000),
                episode_start(2, 1, 2, 70_500),
                episode_end(2, 1, 2, 2, at=71_000),
                episode_start(3, 2, 3, 80_500),
                episode_end(
                    3, 2, "none", 3, result="failed", install="none", at=81_000
                ),
                episode_start(4, 3, 9, 85_000),
                episode_end(4, 3, 4, 9, at=85_500),
                episode_start(5, 4, "none", 86_000),
                episode_end(5, 4, 5, "none", at=86_500),
                episode_start(6, 5, 6, 87_000),
                episode_start(7, 1, 7, 90_100),
                episode_end(7, 1, 2, 7, at=90_500),
                episode_start(8, 1, 8, 95_100),
                episode_end(8, 1, 2, 8, at=95_300),
                episode_start(9, 1, 1, 96_000),
                episode_end(9, 1, 2, 2, at=96_500),
                episode_start(10, 1, 10, 97_000),
                episode_end(
                    10, 1, "none", 10, result="failed", install="none", at=97_500
                ),
                episode_start(11, 1, 11, 98_000),
                episode_end(11, 1, 3, 11, at=98_500),
            ]
        )
        episodes = MODULE.analyze(lines)["owed"]["episodes"]
        classes = {e["episode"]: e["class"] for e in episodes["rows"]}
        self.assertEqual(
            classes,
            {
                1: "satisfied",
                2: "prior_socket",
                3: "not_installed",
                4: "missing_evidence",
                5: "no_reply_owed",
                6: "incomplete",
                7: "unanswered",
                8: "superseded",
                9: "conflict",
                10: "prior_socket",
                11: "prior_socket",
            },
        )
        self.assertEqual(episodes["count"], 11)

    def test_audio_between_the_replaced_and_installed_socket_is_prior_whatever_the_outcome(
        self,
    ):
        lines = (
            reply(
                1,
                60_000,
                61_000,
                accept_socket=2,
                socket=2,
                capture=False,
                terminal="answered",
            )
            + reply(
                2,
                70_000,
                71_000,
                accept_socket=2,
                socket=2,
                capture=False,
                terminal="superseded",
            )
            + [
                episode_start(1, 1, 1, 60_200),
                episode_end(1, 1, 3, 1, at=60_700),
                episode_start(2, 1, 2, 70_200),
                episode_end(2, 1, 3, 2, at=70_700),
            ]
        )
        classes = [
            e["class"] for e in MODULE.analyze(lines)["owed"]["episodes"]["rows"]
        ]
        self.assertEqual(classes, ["prior_socket", "prior_socket"])

    def test_a_reply_carried_through_two_episodes_satisfies_only_the_last(self):
        lines = (
            reply(1, 60_000, 66_000, episodes=[1, 2], accept_socket=3, socket=3)
            + [episode_start(1, 1, 1, 60_500), episode_end(1, 1, 2, 1, at=61_000)]
            + [episode_start(2, 2, 1, 62_000), episode_end(2, 2, 3, 1, at=62_500)]
        )
        rows = MODULE.analyze(lines)["owed"]["episodes"]["rows"]
        self.assertEqual(
            [(e["episode"], e["class"]) for e in rows],
            [(1, "carried"), (2, "satisfied")],
        )
        self.assertIsNone(rows[0]["setup_to_first_output_ms"])
        self.assertIsNotNone(rows[1]["setup_to_first_output_ms"])

    def test_a_later_successful_capture_counts_after_an_earlier_failed_one(self):
        lines = (
            reply(1, 60_000, 63_000, episodes=[1], accept_socket=2, capture=False)
            + [
                line(
                    stage="audio_enqueue_failed",
                    captured=False,
                    turn=1,
                    obligation=1,
                    obligation_outcome="open",
                    owed_start_interview_ms=60_000,
                    interview_ms=60_900,
                    observed_ms=900,
                    origin="prompt",
                ),
                captured(
                    origin="prompt",
                    turn=1,
                    obligation=1,
                    obligation_outcome="open",
                    owed_start_interview_ms=60_000,
                    interview_ms=63_000,
                    observed_ms=960,
                    first_accept_socket=2,
                ),
            ]
            + [episode_start(1, 1, 1, 60_200), episode_end(1, 1, 2, 1, at=60_700)]
        )
        report = MODULE.analyze(lines)
        self.assertEqual(report["turns"][0]["owed"]["state"], "captured")
        self.assertEqual(report["owed"]["episodes"]["classes"], {"satisfied": 1})

    def test_origin_to_native_capture_is_tracked_apart_from_setup_to_first_output(self):
        lines = reply(1, 60_100, 63_000, episodes=[1]) + [
            episode_start(1, 1, 1, 60_500),
            episode_end(1, 1, 2, 1, at=61_000),
        ]
        owed = MODULE.analyze(lines)["owed"]
        row = owed["episodes"]["rows"][0]
        # First output 3_500 ms after the origin; installed 500 ms after 60_500.
        self.assertEqual(row["setup_to_first_output_ms"], 60_100 + 3_500 - 61_000)
        tracks = owed["tracks"]
        self.assertEqual(tracks["origin_to_native_capture_ms"]["p50_ms"], 3_730)
        self.assertEqual(tracks["setup_to_first_output_ms"]["p50_ms"], 2_600)
        self.assertEqual(tracks["setup_to_first_output_unknown"], 0)

    def test_an_unknown_install_leaves_setup_to_first_output_unknown(self):
        lines = reply(1, 60_100, 63_000, episodes=[1]) + [
            episode_start(1, 1, 1, "none"),
            episode_end(1, 1, 2, 1, at="none"),
        ]
        tracks = MODULE.analyze(lines)["owed"]["tracks"]
        self.assertEqual(tracks["setup_to_first_output_ms"]["count"], 0)
        self.assertEqual(tracks["setup_to_first_output_unknown"], 1)

    def test_conflicting_copies_give_the_same_result_in_either_order(self):
        base = reply(1, 60_100, 63_000, episodes=[1])
        start = episode_start(1, 1, 1, 60_500)
        early = episode_end(1, 1, 2, 1, at=61_000, install=500)
        late = episode_end(1, 1, 2, 1, at=61_000, install=3_000)
        one = MODULE.analyze(base + [start, early, late])["owed"]["episodes"]
        two = MODULE.analyze(base + [start, late, early])["owed"]["episodes"]
        self.assertEqual(one["classes"], {"conflict": 1})
        self.assertEqual(one["classes"], two["classes"])
        self.assertIsNone(one["rows"][0]["setup_to_first_output_ms"])
        self.assertIsNone(two["rows"][0]["setup_to_first_output_ms"])

    def test_a_cut_log_leaves_the_episode_unresolved_not_unanswered(self):
        lines = reply(
            1, 60_100, None, episodes=[1], accept_socket=2, capture=False, terminal=None
        ) + [episode_start(1, 1, 1, 60_500), episode_end(1, 1, 2, 1, at=61_000)]
        classes = MODULE.analyze(lines)["owed"]["episodes"]["classes"]
        self.assertEqual(classes, {"unresolved": 1})

    def test_a_stalled_reply_stays_out_of_the_origin_to_capture_track(self):
        lines = reply(1, 10_000, 20_000) + reply(2, 30_000, 40_000)
        lines = [
            text.replace('"server_total_ms": 3730', '"server_total_ms": 20000')
            if '"turn": 2' in text
            else text
            for text in lines
        ]
        report = MODULE.analyze(lines)
        track = report["owed"]["tracks"]["origin_to_native_capture_ms"]
        self.assertEqual(report["stalls"], {"count": 1, "of": 2})
        self.assertEqual((track["count"], track["max_ms"]), (1, 3_730))

    def test_a_scope_with_a_verified_zero_exposure_is_not_unverified(self):
        lines = reply(1, 10_000, 20_000, room="a") + [clock(0, 0, room="b")]
        owed = MODULE.analyze(lines)["owed"]
        reasons = {s["room"]: s["exposure_unverified"] for s in owed["scopes"]}
        self.assertEqual(reasons, {"a": "no_clock_samples", "b": None})
        self.assertEqual(owed["unverified_scopes"], 1)

    def test_unknown_setup_to_first_output_counts_only_where_it_applies(self):
        lines = (
            reply(1, 60_100, 63_000, episodes=[1])
            + reply(2, 70_000, 73_000, episodes=[2])
            + [
                episode_start(1, 1, 1, 60_500),
                episode_end(1, 1, 2, 1, at=61_000),
                episode_start(2, 2, 2, 70_500),
                episode_end(
                    2, 2, "none", 2, result="failed", install="none", at=71_000
                ),
            ]
        )
        tracks = MODULE.analyze(lines)["owed"]["tracks"]
        self.assertEqual(tracks["setup_to_first_output_ms"]["count"], 1)
        self.assertEqual(tracks["setup_to_first_output_unknown"], 0)
        self.assertEqual(tracks["setup_to_first_output_not_applicable"], 1)

    def test_provider_event_replies_stay_out_of_the_origin_to_capture_track(self):
        provider = [
            captured(turn=n, origin="provider_event", server_total_ms=5, elapsed_ms=5)
            for n in range(10, 20)
        ]
        report = MODULE.analyze(reply(1, 10_000, 20_000) + provider)
        tracks = report["owed"]["tracks"]["origin_to_native_capture_ms"]
        self.assertEqual((tracks["count"], tracks["p50_ms"]), (1, 3_730))

    def test_a_disagreeing_copy_is_a_conflict_not_a_second_episode(self):
        lines = [
            episode_start(1, 1, 1, 60_500),
            episode_start(1, 1, 2, 60_500),
            episode_end(1, 1, 2, 1, at=61_000),
        ]
        report = MODULE.analyze(lines)
        self.assertEqual(report["owed"]["episodes"]["count"], 1)
        self.assertEqual(report["lifecycle_conflicts"], 1)


class ResourceTests(unittest.TestCase):
    def test_pressure_ingress_quarantine_omission_and_sockets_are_counted(self):
        lines = (
            reply(1, 10_000, 20_000, episodes=[1])
            + [pressure(socket=1), pressure(socket=2, end="terminal")]
            + [
                episode_start(1, 1, 1, 10_500),
                episode_end(1, 1, 2, 1, at=11_000),
                "recovery ingress: dropped_bytes=10 unconfirmed_bytes=20 unsent_bytes=3 "
                "omitted_contexts=1 omitted_evictions=2 room=r",
                "recovery ingress: dropped_bytes=5 unconfirmed_bytes=0 unsent_bytes=0 "
                "omitted_contexts=0 omitted_evictions=0 room=other room",
                "candidate audio quarantine: bytes=480 room=r",
                "candidate audio quarantine: bytes=20 room=r",
            ]
        )
        report = MODULE.analyze(lines)
        resources = report["resources"]
        self.assertEqual(report["socket_pressure"]["sockets"], 2)
        self.assertEqual(resources["recovery_ingress"]["r"]["omitted_contexts"], 1)
        self.assertEqual(resources["recovery_ingress"]["r"]["omitted_evictions"], 2)
        self.assertEqual(
            resources["recovery_ingress"]["other room"]["dropped_bytes"], 5
        )
        self.assertEqual(resources["audio_quarantine"]["r"], {"lines": 2, "bytes": 500})
        self.assertEqual(report["owed"]["scopes"][0]["sockets"], 2)
        self.assertEqual(resources["sockets_observed"], 2)
        self.assertIsNone(resources["task"])
        # Two ingress lines, one episode end: more lines than ends says so.
        self.assertTrue(resources["ingress_lines_exceed_episode_ends"])
        self.assertFalse(resources["room_counts_exact"])
        text = "\n".join(MODULE.summary_lines(report))
        for expected in (
            "recovery_ingress_omitted_contexts=1",
            "audio_quarantine_bytes=500",
            "sockets_observed=2",
            "task=unknown",
            "owed_s_per_interview_hour=unknown",
        ):
            self.assertIn(expected, text)

    def test_a_copied_log_marks_the_room_counts_as_possibly_doubled(self):
        counts = [
            "recovery ingress: dropped_bytes=0 unconfirmed_bytes=0 unsent_bytes=0 "
            "omitted_contexts=0 omitted_evictions=0 room=r",
            "candidate audio quarantine: bytes=100 room=r",
        ]
        lines = reply(1, 10_000, 20_000, episodes=[1]) + [
            episode_start(1, 1, 1, 10_500),
            episode_end(1, 1, 2, 1, at=11_000),
            *counts,
        ]
        self.assertTrue(MODULE.analyze(lines)["resources"]["room_counts_exact"])
        copied = MODULE.analyze(lines + lines)["resources"]
        self.assertFalse(copied["room_counts_exact"])
        self.assertEqual(copied["audio_quarantine"]["r"]["bytes"], 200)

    def test_a_disagreeing_clock_sample_is_a_conflict(self):
        report = MODULE.analyze(
            reply(1, 1, 2) + [clock(0, 0), clock(1, 60_000), clock(1, 61_000)]
        )
        self.assertEqual(report["lifecycle_conflicts"], 1)
        self.assertIsNone(report["owed"]["owed_s_per_interview_hour"])

    def test_the_rate_says_when_it_is_only_a_lower_bound(self):
        whole = MODULE.analyze(reply(1, 10_000, 20_000) + hour_clock())["owed"]
        self.assertFalse(whole["rate_is_lower_bound"])
        cut = MODULE.analyze(
            reply(1, 10_000, 20_000)
            + reply(2, 30_000, None, capture=False, terminal=None)
            + hour_clock()
        )["owed"]
        self.assertTrue(cut["rate_is_lower_bound"])

    def test_a_legacy_log_summary_gains_no_lines(self):
        text = "\n".join(MODULE.summary_lines(MODULE.analyze([captured()])))
        for name in (
            "owed_",
            "episode",
            "task=",
            "sockets_observed",
            "duplicate_records",
        ):
            self.assertNotIn(name, text)


def task_line(sample, alive, *, elapsed=None, pid=7, session=1, room="r"):
    elapsed = sample * 30_000 if elapsed is None else elapsed
    return (
        f"runtime tasks: pid={pid} session={session} elapsed_ms={elapsed} "
        f"sample={sample} alive_tasks={alive} room={room}"
    )


class RuntimeTaskTests(unittest.TestCase):
    def task(self, extra):
        lines = reply(1, 10_000, 20_000) + hour_clock() + extra
        return MODULE.analyze(lines)["resources"]["task"]

    def test_a_log_with_no_task_lines_has_an_unknown_task_never_zero(self):
        report = MODULE.analyze(reply(1, 10_000, 20_000) + hour_clock())
        self.assertIsNone(report["resources"]["task"])
        text = "\n".join(MODULE.summary_lines(report))
        self.assertIn("task=unknown", text)
        self.assertNotIn("task_alive_max", text)

    def test_samples_the_runtime_could_not_answer_stay_unknown(self):
        task = self.task([task_line(1, "none"), task_line(2, "none")])
        self.assertEqual(task["samples"], 2)
        self.assertEqual(task["known"], 0)
        self.assertEqual(task["unknown"], 2)
        self.assertIsNone(task["alive_max"])
        self.assertIsNone(task["alive_last"])
        report = MODULE.analyze(
            reply(1, 10_000, 20_000)
            + hour_clock()
            + [task_line(1, "none"), task_line(2, "none")]
        )
        text = "\n".join(MODULE.summary_lines(report))
        self.assertIn("task_alive_max=unknown", text)
        self.assertIn("task_alive_last=unknown", text)
        self.assertIn("task_unknown_samples=2", text)

    def test_a_real_zero_is_a_count_and_the_maximum_ignores_unknowns(self):
        task = self.task([task_line(1, 0), task_line(2, "none"), task_line(3, 9)])
        self.assertEqual((task["known"], task["unknown"]), (2, 1))
        self.assertEqual(task["alive_max"], 9)
        self.assertEqual(task["alive_last"], 9)
        zero = self.task([task_line(1, 0)])
        self.assertEqual(zero["alive_max"], 0)
        self.assertEqual(zero["known"], 1)

    def test_the_last_count_follows_the_clock_not_the_line_order(self):
        task = self.task([task_line(2, 8), task_line(1, 3)])
        self.assertEqual(task["alive_last"], 8)

    def test_a_copied_line_is_one_sample_and_a_differing_copy_is_a_conflict(self):
        once = MODULE.analyze(reply(1, 1, 2) + [task_line(1, 4), task_line(1, 4)])
        self.assertEqual(once["resources"]["task"]["samples"], 1)
        self.assertEqual(once["lifecycle_conflicts"], 0)
        twice = MODULE.analyze(reply(1, 1, 2) + [task_line(1, 4), task_line(1, 5)])
        self.assertEqual(twice["lifecycle_conflicts"], 1)

    def test_a_malformed_count_is_rejected_rather_than_read_as_zero(self):
        for bad in ("", "-1", "x", "0x1", "1.5"):
            report = MODULE.analyze(reply(1, 1, 2) + [task_line(1, bad)])
            self.assertIsNone(report["resources"]["task"], bad)
            self.assertGreaterEqual(report["rejected_records"], 1, bad)

    def test_a_task_line_cut_at_the_end_of_a_tail_is_not_trusted(self):
        report = MODULE.analyze(reply(1, 1, 2) + [task_line(1, 4)], tail_open=True)
        self.assertIsNone(report["resources"]["task"])
        self.assertTrue(report["truncated_final"])


def mic_clock(sample, mx, o1, o10, o30, *, elapsed=None, pid=7, session=1, room="r"):
    elapsed = sample * 60_000 if elapsed is None else elapsed
    return (
        f"interview clock: pid={pid} session={session} start_unix_ms=1700000000000 "
        f"elapsed_ms={elapsed} sample={sample} mic_gap_max_ms={mx} "
        f"mic_gap_over_1s={o1} mic_gap_over_10s={o10} mic_gap_over_30s={o30} room={room}"
    )


def mic_final(mx, o1, o10, o30, *, elapsed=3_600_000, pid=7, session=1, room="r"):
    return (
        f"mic gaps: pid={pid} session={session} elapsed_ms={elapsed} "
        f"mic_gap_max_ms={mx} mic_gap_over_1s={o1} mic_gap_over_10s={o10} "
        f"mic_gap_over_30s={o30} room={room}"
    )


class MicGapTests(unittest.TestCase):
    def mic(self, extra, *, base=None):
        lines = reply(1, 10_000, 20_000) + (hour_clock() if base is None else base)
        return MODULE.analyze(lines + extra)["resources"]["mic_gap"]

    def test_a_legacy_log_has_unknown_mic_gaps_never_zero(self):
        report = MODULE.analyze(reply(1, 10_000, 20_000) + hour_clock())
        mic = report["resources"]["mic_gap"]
        # The legacy clock lines parse, and say nothing of the gap.
        self.assertEqual(mic["known"], 0)
        self.assertEqual(mic["unknown"], 2)
        for name in (
            "mic_gap_max_ms",
            "mic_gap_over_1s",
            "mic_gap_over_10s",
            "mic_gap_over_30s",
        ):
            self.assertIsNone(mic[name], name)
        text = "\n".join(MODULE.summary_lines(report))
        self.assertIn("mic_gap_max_ms=unknown", text)
        self.assertIn("mic_gap_over_30s=unknown", text)
        self.assertNotIn("mic_gap_max_ms=0", text)

    def test_a_log_with_no_clock_or_gap_line_has_no_mic_gap_at_all(self):
        report = MODULE.analyze(reply(1, 10_000, 20_000))
        self.assertIsNone(report["resources"]["mic_gap"])

    def test_none_is_unknown_and_a_real_zero_is_a_measurement(self):
        unmeasured = self.mic([mic_clock(1, "none", "none", "none", "none")], base=[])
        self.assertEqual((unmeasured["known"], unmeasured["unknown"]), (0, 1))
        self.assertIsNone(unmeasured["mic_gap_max_ms"])
        zero = self.mic([mic_clock(1, 0, 0, 0, 0)], base=[])
        self.assertEqual(zero["mic_gap_max_ms"], 0)
        self.assertEqual(zero["mic_gap_over_1s"], 0)
        self.assertEqual(zero["known"], 1)

    def test_the_maximum_and_cumulative_counts_come_from_the_largest_reading(self):
        mic = self.mic(
            [
                mic_clock(1, 1500, 1, 0, 0),
                mic_clock(2, 31_000, 3, 2, 1),
                mic_clock(3, "none", "none", "none", "none"),
                mic_final(31_000, 4, 2, 1),
            ]
        )
        self.assertEqual(mic["mic_gap_max_ms"], 31_000)
        self.assertEqual(
            (mic["mic_gap_over_1s"], mic["mic_gap_over_10s"], mic["mic_gap_over_30s"]),
            (4, 2, 1),
        )
        self.assertEqual(mic["known"], 3)
        self.assertEqual(mic["unknown"], 3)
        text = "\n".join(
            MODULE.summary_lines(
                MODULE.analyze(
                    reply(1, 10_000, 20_000)
                    + [mic_clock(2, 31_000, 3, 2, 1), mic_final(31_000, 4, 2, 1)]
                )
            )
        )
        self.assertIn("mic_gap_max_ms=31000", text)
        self.assertIn("mic_gap_over_1s=4", text)

    def test_interviews_add_their_counts_and_share_the_maximum(self):
        mic = self.mic(
            [
                mic_final(2000, 1, 0, 0, session=1),
                mic_final(12_000, 5, 1, 0, session=2),
            ],
            base=[],
        )
        self.assertEqual(mic["mic_gap_max_ms"], 12_000)
        self.assertEqual(mic["mic_gap_over_1s"], 6)
        self.assertEqual(mic["mic_gap_over_10s"], 1)

    def test_a_final_line_does_not_extend_the_exposure_of_the_clock(self):
        clocks = reply(1, 10_000, 20_000) + hour_clock()
        plain = MODULE.analyze(clocks)["owed"]
        with_final = MODULE.analyze(
            clocks + [mic_final(5000, 1, 0, 0, elapsed=ONE_HOUR_MS + 59_000)]
        )["owed"]
        self.assertEqual(plain["exposure_s"], with_final["exposure_s"])

    def test_a_clock_line_in_two_overlapping_files_counts_once(self):
        line = mic_clock(1, 1500, 1, 0, 0)
        self.assertEqual(self.mic([line, line], base=[])["lines"], 1)

    def test_a_copied_final_line_counts_once(self):
        once = self.mic([mic_final(2000, 1, 0, 0), mic_final(2000, 1, 0, 0)], base=[])
        self.assertEqual(once["lines"], 1)

    def test_a_malformed_or_partial_gap_field_is_rejected_not_read_as_zero(self):
        good = mic_clock(1, 1500, 1, 0, 0)
        cases = {
            "negative": good.replace("mic_gap_max_ms=1500", "mic_gap_max_ms=-1"),
            "text": good.replace("mic_gap_over_1s=1", "mic_gap_over_1s=x"),
            "partial": good.replace(" mic_gap_over_10s=0", ""),
            "none_among_numbers": good.replace(
                "mic_gap_over_30s=0", "mic_gap_over_30s=none"
            ),
            "not_nested": mic_clock(1, 40_000, 1, 2, 0),
            "count_without_a_long_enough_gap": mic_clock(1, 500, 2, 0, 0),
            "long_gap_with_no_count": mic_clock(1, 40_000, 1, 1, 0),
        }
        for name, line in cases.items():
            report = MODULE.analyze(reply(1, 1, 2) + [line])
            self.assertIsNone(report["resources"]["mic_gap"], name)
            self.assertGreaterEqual(report["rejected_records"], 1, name)
        final = mic_final(1, 0, 0, 0).replace(" mic_gap_over_30s=0", "")
        report = MODULE.analyze(reply(1, 1, 2) + [final])
        self.assertGreaterEqual(report["rejected_records"], 1)

    def test_a_final_line_cut_at_the_end_of_a_tail_is_not_trusted(self):
        report = MODULE.analyze(
            reply(1, 1, 2) + [mic_final(5000, 1, 0, 0)], tail_open=True
        )
        self.assertIsNone(report["resources"]["mic_gap"])
        self.assertTrue(report["truncated_final"])

    def test_the_gap_fields_do_not_change_the_clock_samples_they_ride(self):
        report = MODULE.analyze(
            reply(1, 10_000, 20_000)
            + [mic_clock(0, "none", "none", "none", "none", elapsed=0)]
            + [mic_clock(60, 900, 0, 0, 0, elapsed=ONE_HOUR_MS)]
        )
        self.assertEqual(report["owed"]["exposure_s"], 3600.0)


class ControlAdapterTests(unittest.TestCase):
    def files(self, directory, control, candidate):
        paths = []
        for name, lines in (("control.log", control), ("candidate.log", candidate)):
            path = Path(directory) / name
            path.write_text("\n".join(lines) + "\n", encoding="utf-8")
            paths.append(str(path))
        return paths

    def adapted(self, control, candidate, sha=CONTROL_SHA, *extra):
        with tempfile.TemporaryDirectory() as directory:
            old, new = self.files(directory, control, candidate)
            return run(
                "--compare", old, "--control-adapter", sha, *extra, new, stdin=""
            )

    def test_a_legacy_control_is_compared_on_observable_metrics_only(self):
        candidate = reply(1, 10_000, 20_000) + reply(2, 30_000, 40_000) + hour_clock()
        result = self.adapted(
            [captured(), captured(turn=2)], candidate, CONTROL_SHA, "--json"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["control_revision"], CONTROL_SHA)
        self.assertIn("origins", report["comparison"])
        unobservable = " ".join(report["unobservable"])
        for name in ("owed_gap_s", "interview-hour", "episodes", "pressure", "task"):
            self.assertIn(name, unobservable)
        self.assertEqual(report["candidate"]["owed"]["count"], 2)
        text = self.adapted([captured()], candidate).stdout
        self.assertIn("Not observable in the control", text)

    def test_a_candidate_missing_lifecycle_ids_fails_without_a_fallback(self):
        legacy = [captured(), captured(turn=2)]
        mixed = reply(1, 10_000, 20_000) + [captured(turn=2)]
        for name, candidate in (("legacy", legacy), ("mixed", mixed)):
            with self.subTest(name):
                result = self.adapted([captured()], candidate)
                self.assertEqual(result.returncode, 1)
                self.assertEqual(result.stdout, "")
                self.assertIn("lacks required lifecycle ids", result.stderr)

    def test_the_adapter_reports_the_legacy_metrics_it_lists_as_observable(self):
        candidate = reply(1, 10_000, 20_000) + hour_clock()
        result = self.adapted(
            [captured(), captured(turn=2)], candidate, CONTROL_SHA, "--json"
        )
        report = json.loads(result.stdout)
        self.assertEqual(report["control"]["replies"], 2)
        self.assertEqual(report["control"]["captured"], 2)
        self.assertEqual(report["candidate_legacy"]["captured"], 1)
        self.assertEqual(report["control"]["origin_to_first_output_ms"]["count"], 2)
        self.assertEqual(
            report["candidate_legacy"]["origin_to_first_output_ms"]["p50_ms"], 3_500
        )
        self.assertEqual(
            report["control"]["origin_to_native_capture_ms"]["last_voiced_pcm"][
                "count"
            ],
            2,
        )

    def test_the_adapter_names_the_side_with_no_replies(self):
        result = self.adapted([captured()], ["nothing here"])
        self.assertEqual((result.returncode, result.stdout), (1, ""))
        self.assertIn("candidate log has no replies", result.stderr)
        result = self.adapted(["nothing here"], reply(1, 10_000, 20_000))
        self.assertIn("control log has no replies", result.stderr)

    def test_first_output_metric_leaves_out_provider_event_replies(self):
        provider = [
            captured(turn=n, origin="provider_event", first_provider_output_ms=1)
            for n in range(10, 20)
        ]
        result = self.adapted(
            [captured()], reply(1, 10_000, 20_000) + provider, CONTROL_SHA, "--json"
        )
        report = json.loads(result.stdout)
        self.assertEqual(
            report["candidate_legacy"]["origin_to_first_output_ms"]["p50_ms"], 3_500
        )

    def test_a_candidate_with_a_rejected_record_fails_without_a_fallback(self):
        # A record with an obligation and no outcome is rejected by the parser,
        # so the id check would otherwise never see it.
        broken = reply(2, 30_000, 40_000)
        broken = [
            text.replace('"obligation_outcome": "open"', '"obligation_outcome": null')
            for text in broken
        ]
        result = self.adapted([captured()], reply(1, 10_000, 20_000) + broken)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertIn("rejected records", result.stderr)

    def test_a_candidate_with_no_owed_origin_record_has_nothing_to_join(self):
        events = [line(stage="turn_complete", origin="provider_event", turn=1)]
        result = self.adapted([captured()], events)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no reply owed to a local input", result.stderr)

    def test_a_control_log_that_carries_lifecycle_ids_is_not_that_legacy_revision(self):
        result = self.adapted(reply(1, 10_000, 20_000), reply(1, 10_000, 20_000))
        self.assertEqual(result.returncode, 1)
        self.assertIn("control log carries lifecycle ids", result.stderr)

    def test_an_unpinned_control_revision_is_refused(self):
        candidate = reply(1, 10_000, 20_000)
        for sha in ("0" * 40, "1ceda0ef", ""):
            with self.subTest(sha=sha):
                result = self.adapted([captured()], candidate, sha)
                self.assertEqual(result.returncode, 1)
                self.assertIn("no adapter", result.stderr)

    def test_the_adapter_needs_a_control_log(self):
        result = run("--control-adapter", CONTROL_SHA, stdin="\n".join(reply(1, 1, 2)))
        self.assertEqual(result.returncode, 2)

    def test_a_candidate_without_replies_fails(self):
        result = self.adapted([captured()], ["nothing here"])
        self.assertEqual(result.returncode, 1)

    def test_a_log_from_before_the_lifecycle_records_reads_as_a_legacy_control(self):
        # What the control revision writes: native captures with no obligation,
        # episode or socket identity on any reply.
        legacy = [
            captured(turn=turn, server_total_ms=1_500 + 100 * turn)
            for turn in range(1, 6)
        ]
        report = MODULE.analyze(legacy)
        self.assertEqual(report["rejected_records"], 0)
        self.assertEqual(len(report["turns"]), 5)
        # No lifecycle evidence at all: every owed-origin reply lacks its ids,
        # so as a candidate it fails, and as a control it supplies no owed time.
        self.assertEqual(report["owed"]["count"], 0)
        self.assertEqual(report["owed"]["missing_identity"], len(report["turns"]))
        self.assertTrue(report["identity_gaps"])
        self.assertIsNone(report["owed"]["owed_s_per_interview_hour"])
        candidate = reply(1, 10_000, 20_000) + hour_clock()
        adapted = MODULE.adapt_control(CONTROL_SHA, report, MODULE.analyze(candidate))
        self.assertIn("owed_gap_s", " ".join(adapted["unobservable"]))
        with self.assertRaises(MODULE.AdapterRefused):
            MODULE.adapt_control(CONTROL_SHA, report, report)


def arm_lines(sessions):
    """One arm's log. Each session maps to its replies in order: ms beyond the
    window, "failed" for a reply that never reached capture, or "missing" for a
    captured reply that recorded no first provider output."""
    out = []
    for session, replies in sessions.items():
        for turn, reply in enumerate(replies, 1):
            if reply == "failed":
                out.append(
                    line(
                        stage="native_audio_cancelled",
                        session=session,
                        turn=turn,
                        captured=False,
                    )
                )
            elif reply == "missing":
                out.append(
                    line(
                        stage="native_audio_capture",
                        session=session,
                        turn=turn,
                        captured=True,
                        elapsed_ms=3_700,
                        server_total_ms=3_700,
                    )
                )
            else:
                out.append(
                    captured(
                        session=session,
                        turn=turn,
                        server_total_ms=3_000 + reply,
                        elapsed_ms=3_000 + reply,
                        first_provider_output_ms=3_000 + reply - 100,
                        provider_voice_end_ms=3_100,
                    )
                )
    return out


def paired(before, after):
    return MODULE.paired_report(
        MODULE.analyze(arm_lines(before)), MODULE.analyze(arm_lines(after))
    )


def ladder(start, step=100, count=21):
    return [start + step * i for i in range(count)]


PINNED_INTERVAL = [290, 792.5]


def uneven_sessions():
    """Six sessions whose change differs in size and sign."""
    shifts = (-400, 300, 50, 700, -150, 500)
    before = {s: ladder(1_000 + 150 * s) for s in range(1, 7)}
    after = {
        s: ladder(1_000 + 150 * s + shift, step=100 + 10 * (s % 3))
        for s, shift in zip(range(1, 7), shifts)
    }
    return before, after


class PairedRegressionTests(unittest.TestCase):
    def test_p95_of_known_quantiles_is_reported_per_session_and_pooled(self):
        # 21 values: rank 0.95 * 20 = 19, exactly the 20th value.
        report = paired({1: ladder(1_000)}, {1: ladder(1_500)})["metrics"]
        entry = report["beyond_window_ms"]
        self.assertEqual(
            entry["sessions"],
            [
                {
                    "session": "r/s1",
                    "before_n": 21,
                    "after_n": 21,
                    "before_p95_ms": 2_900,
                    "after_p95_ms": 3_400,
                    "delta_ms": 500,
                }
            ],
        )
        self.assertEqual(entry["arms"]["before"]["p95_ms"], 2_900)
        self.assertEqual(entry["pooled_p95_shift_ms"], 500)
        self.assertEqual(entry["median_session_delta_ms"], 500)
        # One pair has nothing to resample.
        self.assertIsNone(entry["interval"])
        # Every predeclared metric is reported, each from its own field.
        self.assertEqual(
            set(report),
            {
                "beyond_window_ms",
                "origin_to_native_capture_ms",
                "origin_to_first_output_ms",
            },
        )
        capture = report["origin_to_native_capture_ms"]["arms"]["before"]
        self.assertEqual(capture["p95_ms"], 5_900)
        first = report["origin_to_first_output_ms"]["arms"]["before"]
        self.assertEqual(first["p95_ms"], 5_800)

    def test_unequal_session_sizes_are_counted_per_session_not_pooled_away(self):
        before = {1: ladder(1_000), 2: [5_000, 6_000]}
        entry = paired(before, before)["metrics"]["beyond_window_ms"]
        self.assertEqual(
            [(s["session"], s["before_n"]) for s in entry["sessions"]],
            [("r/s1", 21), ("r/s2", 2)],
        )
        # Session 2's p95 sits nine tenths of the way from 5000 to 6000; the
        # pool of 23 puts its p95 at rank 20.9, nine tenths from 3000 to 5000.
        self.assertEqual(entry["sessions"][1]["before_p95_ms"], 5_950)
        self.assertEqual(entry["arms"]["before"]["p95_ms"], 4_800)
        self.assertEqual(entry["arms"]["before"]["samples"]["valid"], 23)
        self.assertEqual(entry["pairs"]["usable"], 2)

    def test_identical_samples_give_no_shift_and_a_zero_width_interval(self):
        same = {s: [1_000] * 8 for s in (1, 2, 3, 4)}
        entry = paired(same, same)["metrics"]["beyond_window_ms"]
        self.assertEqual(entry["pooled_p95_shift_ms"], 0)
        self.assertEqual(entry["interval"]["ci95_ms"], [0, 0])
        self.assertEqual({s["delta_ms"] for s in entry["sessions"]}, {0})

    def test_all_failures_leave_no_percentile_and_no_pair(self):
        failed = {1: ["failed"] * 3, 2: ["failed"] * 2}
        entry = paired({1: ladder(1_000), 2: ladder(1_000)}, failed)["metrics"][
            "beyond_window_ms"
        ]
        after = entry["arms"]["after"]
        self.assertEqual(
            after["samples"], {"of": 5, "valid": 0, "failed": 5, "missing": 0}
        )
        self.assertEqual(after["failure_rate"], 1.0)
        self.assertIsNone(after["p95_ms"])
        self.assertEqual(entry["pairs"]["matched"], 2)
        self.assertEqual(entry["pairs"]["usable"], 0)
        self.assertEqual(entry["pairs"]["no_valid_sample"], 2)
        self.assertIsNone(entry["pooled_p95_shift_ms"])
        self.assertIsNone(entry["interval"])

    def test_failed_and_missing_samples_stay_in_the_denominator(self):
        both = {1: [1_000, 1_100, 1_200, "failed", "missing"]}
        report = paired(both, both)["metrics"]
        entry = report["beyond_window_ms"]["arms"]["before"]
        # A captured reply with no first provider output still has a total.
        self.assertEqual(
            entry["samples"], {"of": 5, "valid": 4, "failed": 1, "missing": 0}
        )
        self.assertEqual(entry["failure_rate"], 0.2)
        first = report["origin_to_first_output_ms"]["arms"]["before"]
        self.assertEqual(
            first["samples"], {"of": 5, "valid": 3, "failed": 1, "missing": 1}
        )
        self.assertEqual(first["failure_rate"], 0.4)

    def test_a_session_present_in_one_arm_is_counted_not_paired(self):
        entry = paired(
            {1: ladder(1_000), 2: ladder(1_000)}, {2: ladder(1_200), 3: ladder(1_200)}
        )["metrics"]["beyond_window_ms"]
        self.assertEqual(
            entry["pairs"],
            {
                "matched": 1,
                "usable": 1,
                "no_valid_sample": 0,
                "only_before": 1,
                "only_after": 1,
            },
        )
        self.assertEqual([s["session"] for s in entry["sessions"]], ["r/s2"])
        # Unpaired sessions still count in their own arm's accounting.
        self.assertEqual(entry["arms"]["before"]["sessions"], 2)
        self.assertEqual(entry["arms"]["after"]["samples"]["valid"], 42)

    def test_three_pairs_stay_a_descriptive_screen_with_no_verdict(self):
        before = {s: ladder(2_000) for s in (1, 2, 3)}
        # Far more than a 30% cut, with a zero-width interval.
        after = {s: ladder(500) for s in (1, 2, 3)}
        report = paired(before, after)
        entry = report["metrics"]["beyond_window_ms"]
        self.assertEqual(entry["pairs"]["usable"], 3)
        self.assertEqual(entry["interval"]["ci95_ms"], [-1_500, -1_500])
        self.assertEqual(report["verdict"], "descriptive-inconclusive")
        text = MODULE.render_paired(report)
        self.assertIn("DESCRIPTIVE regression screen, inconclusive", text)
        self.assertIn("30% latency target is not evaluated", text)
        self.assertIn("session-cluster", text)
        self.assertIn("seed 20260101", text)
        self.assertIn("matched=3 usable=3", text)
        self.assertIsNone(re.search(r"\b(PASS|ADOPT|MET|HEALTHY|SLOW)\b", text))
        self.assertFalse([key for key in report if "adopt" in key or "target" in key])

    def test_session_cluster_resampling_is_not_frame_resampling(self):
        # Each session's frames are copies of one value: one observation per
        # session, however many frames repeat it.
        before = {s: [base] * 50 for s, base in ((1, 1_000), (2, 2_000), (3, 3_000))}
        after = {
            s: [base + gap] * 50
            for s, base, gap in ((1, 1_000, 0), (2, 2_000, 500), (3, 3_000, 1_000))
        }
        entry = paired(before, after)["metrics"]["beyond_window_ms"]
        low, high = entry["interval"]["ci95_ms"]

        def frame_interval():
            """What an implementation that pools frames would report."""
            rng = random.Random(MODULE.PAIRED_SEED)
            b = [v for s in before.values() for v in s]
            a = [v for s in after.values() for v in s]
            shifts = sorted(
                MODULE.percentile(sorted(rng.choices(a, k=len(a))), 0.95)
                - MODULE.percentile(sorted(rng.choices(b, k=len(b))), 0.95)
                for _ in range(MODULE.PAIRED_RESAMPLES)
            )
            return MODULE.percentile(shifts, 0.025), MODULE.percentile(shifts, 0.975)

        naive_low, naive_high = frame_interval()
        # 150 correlated frames pretend to be 150 samples, so the pooled
        # interval is far narrower than three sessions can support.
        self.assertLessEqual((naive_high - naive_low) * 4, high - low)
        self.assertGreaterEqual(high - low, 1_000)

    def test_the_interval_is_fixed_by_the_seed_and_pinned(self):
        before, after = uneven_sessions()
        first = paired(before, after)["metrics"]["beyond_window_ms"]
        again = paired(before, after)["metrics"]["beyond_window_ms"]
        self.assertEqual(first, again)
        self.assertEqual(first["interval"]["seed"], 20_260_101)
        self.assertEqual(first["interval"]["resamples"], 2000)
        self.assertEqual(first["interval"]["ci95_ms"], PINNED_INTERVAL)
        # The seed is what fixes it: few resamples show the seed's effect that
        # the full count's saturated percentiles can hide.
        saved = MODULE.PAIRED_SEED, MODULE.PAIRED_RESAMPLES
        try:
            MODULE.PAIRED_RESAMPLES = 15
            same = [
                paired(before, after)["metrics"]["beyond_window_ms"]["interval"]
                for _ in range(2)
            ]
            MODULE.PAIRED_SEED += 1
            other = paired(before, after)["metrics"]["beyond_window_ms"]["interval"]
        finally:
            MODULE.PAIRED_SEED, MODULE.PAIRED_RESAMPLES = saved
        self.assertEqual(same[0], same[1])
        self.assertNotEqual(other["ci95_ms"], same[0]["ci95_ms"])

    def test_the_cli_is_deterministic_across_processes(self):
        before, after = uneven_sessions()
        with tempfile.TemporaryDirectory() as scratch:
            old = Path(scratch) / "before.log"
            old.write_text("\n".join(arm_lines(before)) + "\n")
            stdin = "\n".join(arm_lines(after)) + "\n"
            runs = [run("--paired", str(old), "--json", stdin=stdin) for _ in range(3)]
            self.assertEqual([r.returncode for r in runs], [0, 0, 0], runs[0].stderr)
            self.assertEqual(len({r.stdout for r in runs}), 1)
            interval = json.loads(runs[0].stdout)["metrics"]["beyond_window_ms"][
                "interval"
            ]
            self.assertEqual(interval["ci95_ms"], PINNED_INTERVAL)
            text = run("--paired", str(old), stdin=stdin)
            self.assertIn("matched=6 usable=6", text.stdout)
            both = run("--paired", str(old), "--compare", str(old), stdin=stdin)
            self.assertEqual(both.returncode, 2)
            empty = run("--paired", str(old), stdin="x\n")
            self.assertEqual(empty.returncode, 1)
            self.assertIn("log under test has no replies", empty.stderr)
            self.assertIn("median of per-session deltas", text.stdout)


class RecoverySeriesTests(unittest.TestCase):
    """Both readers consume this reduced synthetic trace, never a private run log."""

    def trace(self):
        return (
            SCRIPT.parent.parent / "tests/fixtures/gemini-recovery-series.txt"
        ).read_text()

    def test_six_rotations_preserve_one_carried_reply_and_seven_reader_summaries(self):
        report = MODULE.analyze(self.trace().splitlines())
        self.assertEqual(report["rejected_records"], 0)
        self.assertEqual(report["lifecycle_conflicts"], 0)
        self.assertEqual(report["owed"]["count"], 6)
        self.assertEqual(report["owed"]["states"], {"captured": 5, "superseded": 1})
        episodes = report["owed"]["episodes"]
        self.assertEqual(episodes["count"], 6)
        self.assertEqual(episodes["classes"], {"no_reply_owed": 5, "satisfied": 1})
        self.assertEqual(
            [row["new_socket"] for row in episodes["rows"]], list(range(2, 8))
        )
        self.assertEqual(report["socket_pressure"]["sockets"], 7)
        self.assertEqual(report["socket_pressure"]["blocked_sends"], 0)
        self.assertEqual(report["socket_pressure"]["read_idle_ends"], 0)
        self.assertEqual(report["socket_pressure"]["high_water_max"], 27)
        readers = report["socket_pressure"]["rows"]
        self.assertEqual([r["socket"] for r in readers], list(range(1, 8)))
        self.assertTrue(all(r["received"] == r["consumed"] for r in readers))
        self.assertEqual(
            [r["end"] for r in readers], ["replacement"] * 6 + ["terminal"]
        )

    def test_duplicate_sources_do_not_double_count_the_series(self):
        once = MODULE.analyze(self.trace().splitlines())
        twice = MODULE.analyze((self.trace() * 2).splitlines())
        for key in ("count", "states", "exposure_s", "gap_s_total", "episodes"):
            self.assertEqual(once["owed"][key], twice["owed"][key], key)
        self.assertEqual(twice["socket_pressure"]["sockets"], 7)
        self.assertEqual(twice["lifecycle_conflicts"], 0)

    def test_every_missing_end_remains_unresolved_despite_other_successful_swaps(self):
        for episode in range(1, 7):
            with self.subTest(episode=episode):
                lines = [
                    s
                    for s in self.trace().splitlines()
                    if not (
                        s.startswith("replacement end:") and f" episode={episode} " in s
                    )
                ]
                rows = MODULE.analyze(lines)["owed"]["episodes"]["rows"]
                self.assertEqual(len(rows), 6)
                self.assertEqual(
                    next(r for r in rows if r["episode"] == episode)["class"],
                    "incomplete",
                )

    def test_prior_socket_audio_does_not_satisfy_the_carried_reply(self):
        lines = []
        for text in self.trace().splitlines():
            if text.startswith("codetrial latency "):
                record = json.loads(text[len("codetrial latency ") :])
                if record.get("obligation") == 2:
                    record.update(socket=2, first_accept_socket=2)
                    text = "codetrial latency " + json.dumps(record)
            lines.append(text)
        rows = MODULE.analyze(lines)["owed"]["episodes"]["rows"]
        self.assertEqual(
            next(r for r in rows if r["episode"] == 2)["class"], "prior_socket"
        )

    def test_clock_scopes_stalls_and_initial_unknown_gap_stay_distinct(self):
        report = MODULE.analyze(self.trace().splitlines())
        self.assertEqual(report["owed"]["exposure_s"], 3608)
        self.assertEqual(report["stall_ms"], 5000)
        self.assertEqual(report["stalls"], {"count": 2, "of": 5})
        self.assertEqual(report["beyond_window"]["count"], 3)
        self.assertEqual(report["beyond_window"]["p95_ms"], 1000)
        self.assertEqual(report["beyond_window_with_stalls"]["count"], 5)
        self.assertEqual(report["beyond_window_with_stalls"]["max_ms"], 8000)
        mic = report["resources"]["mic_gap"]
        self.assertEqual((mic["known"], mic["unknown"]), (2, 1))
        self.assertEqual(mic["mic_gap_max_ms"], 27)
        self.assertEqual(mic["mic_gap_over_1s"], 0)

    def test_strict_cli_reads_the_shared_trace_and_rejects_its_cut_final_record(self):
        for body, expected in ((self.trace(), 0), (self.trace().rstrip("\n"), 1)):
            with self.subTest(complete=expected == 0):
                result = run("--json", "--strict", stdin=body)
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual(
                    json.loads(result.stdout)["truncated_final"], expected == 1
                )


if __name__ == "__main__":
    unittest.main()
