"""Live-review MCP tool handlers extracted from server.call_tool."""

from __future__ import annotations

import json
from pathlib import Path

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    optional_string_argument,
    required_string_argument,
)
from devcouncil.live.cards import filter_cards, get_card, load_cards
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.live.summary import live_review_summary


async def handle_live_review(root: Path, arguments: dict) -> list[TextContent]:
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    return [TextContent(
        type="text",
        text=json.dumps(live_review_summary(root, task_id=task_id), indent=2),
    )]


async def handle_live_cards(root: Path, arguments: dict) -> list[TextContent]:
    task_id = optional_string_argument(arguments, "task_id")
    status = optional_string_argument(arguments, "status")
    verdict = optional_string_argument(arguments, "verdict")
    client = optional_string_argument(arguments, "client")
    for arg_name, value in [
        ("task_id", task_id),
        ("status", status),
        ("verdict", verdict),
        ("client", client),
    ]:
        if value == "":
            return error_text(f"{arg_name} must be a string", code="invalid_arguments", argument=arg_name)

    limit = int_argument(arguments, "limit", 20, minimum=1, maximum=200)
    filtered, filter_error, argument = filter_cards(
        load_cards(root),
        task_id=task_id,
        status=status,
        verdict=verdict,
        client=client,
    )
    if filter_error:
        return error_text(filter_error, code="invalid_arguments", argument=argument)

    total = len(filtered)
    return [TextContent(
        type="text",
        text=json.dumps({
            "cards": [card.model_dump() for card in filtered[:limit]],
            "filters": {
                "task_id": task_id,
                "status": status,
                "verdict": verdict,
                "client": client,
            },
            "limit": limit,
            "total": total,
        }, indent=2),
    )]


async def handle_live_repair_prompt(root: Path, arguments: dict) -> list[TextContent]:
    card_id, arg_error = required_string_argument(arguments, "card_id")
    if arg_error:
        return arg_error
    assert card_id is not None
    card = get_card(root, card_id)
    if not card:
        return error_text(f"Critique card {card_id} not found.", code="not_found", card_id=card_id)
    return [TextContent(
        type="text",
        text=json.dumps({
            "card": card.model_dump(),
            "prompt": build_live_repair_prompt(root, card),
        }, indent=2),
    )]


async def handle_live_repair_all(root: Path, arguments: dict) -> list[TextContent]:
    task_id = optional_string_argument(arguments, "task_id")
    if task_id == "":
        return error_text("task_id must be a string", code="invalid_arguments", argument="task_id")
    summary = live_review_summary(root, task_id=task_id)
    cards = [
        get_card(root, item["id"])
        for item in summary["blocking_cards"]
        if isinstance(item.get("id"), str)
    ]
    resolved_cards = [card for card in cards if card is not None]
    return [TextContent(
        type="text",
        text=json.dumps({
            "scope_task_id": summary["scope_task_id"],
            "cards": [card.model_dump() for card in resolved_cards],
            "prompt": build_bulk_live_repair_prompt(root, resolved_cards),
        }, indent=2),
    )]
