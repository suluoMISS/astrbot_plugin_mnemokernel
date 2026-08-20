"""Small deterministic helpers for the event-driven diary trigger."""

from __future__ import annotations

from datetime import date, datetime, timedelta


def due_journal_date(now: datetime, daily_hour: int, daily_minute: int) -> date:
    """Return the latest day whose scheduled diary window has elapsed.

    The diary for a calendar day is due on the following day at the configured
    local time. Before today's trigger, the most recently due diary is therefore
    two calendar days behind.
    """

    if now.tzinfo is None or now.utcoffset() is None:
        raise ValueError("now must be timezone-aware")
    if not 0 <= daily_hour <= 23 or not 0 <= daily_minute <= 59:
        raise ValueError("daily trigger time is invalid")
    trigger = now.replace(
        hour=daily_hour,
        minute=daily_minute,
        second=0,
        microsecond=0,
    )
    days_back = 1 if now >= trigger else 2
    return now.date() - timedelta(days=days_back)
