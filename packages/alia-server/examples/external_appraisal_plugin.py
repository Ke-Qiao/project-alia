#!/usr/bin/env python3
"""Minimal external expression appraisal plugin.

The server writes one ExpressionPluginRequest JSON object to stdin and expects
one ExpressionPluginResponse JSON object on stdout. This example stays local
and deterministic, but the same boundary can call an LLM/classifier service.
"""

import json
import sys


def deltas(**overrides):
    base = {
        "valence": 0.0,
        "arousal": 0.0,
        "calmness": 0.0,
        "engagement": 0.0,
        "fatigue": 0.0,
        "battery_stress": 0.0,
        "thermal_stress": 0.0,
        "sleepiness": 0.0,
        "interaction_load": 0.0,
        "trust": 0.0,
        "familiarity": 0.0,
    }
    base.update(overrides)
    return base


def event(kind, confidence, scope, deltas_value, summary):
    return {
        "kind": kind,
        "source": "classifier",
        "target": "dialogue",
        "confidence": confidence,
        "scope": scope,
        "deltas": deltas_value,
        "evidence_summary": summary,
    }


def appraise_user_input(context):
    text = context.get("user_text", "")
    lowered = text.lower()
    events = []

    if any(marker in text for marker in ("真厉害啊", "真行啊")) and any(
        marker in text for marker in ("答错", "错了", "搞砸")
    ):
        events.append(
            event(
                "external_sarcasm_or_dissatisfaction",
                0.78,
                "session",
                deltas(valence=-0.18, arousal=0.08, engagement=-0.08),
                "External classifier detected sarcastic dissatisfaction.",
            )
        )
    elif any(marker in text for marker in ("谢谢", "厉害", "good job", "thanks")):
        events.append(
            event(
                "external_positive_feedback",
                0.72,
                "session",
                deltas(valence=0.12, engagement=0.08, trust=0.01),
                "External classifier detected positive feedback.",
            )
        )

    if any(marker in lowered for marker in ("don't move", "be brief", "简洁", "别动")):
        events.append(
            event(
                "external_minimal_expression_preference",
                0.82,
                "turn",
                deltas(calmness=0.08, arousal=-0.08),
                "User requested minimal expression.",
            )
        )

    return {"type": "appraisal_events", "events": events}


def main():
    request = json.loads(sys.stdin.read() or "{}")
    request_type = request.get("type")

    if request_type == "health_check":
        response = {"type": "health", "ok": True, "message": "demo plugin ready"}
    elif request_type == "appraise_user_input":
        response = appraise_user_input(request.get("context", {}))
    else:
        response = {"type": "noop"}

    json.dump(response, sys.stdout, ensure_ascii=False)


if __name__ == "__main__":
    main()
