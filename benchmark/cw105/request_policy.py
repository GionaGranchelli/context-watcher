"""Validate the provider request shape that the execution envelope covers."""
import json

from budget import BudgetExceeded


def check_request(body, *, model, max_output_tokens):
    try:
        request = json.loads(body)
    except (ValueError, UnicodeError) as error:
        raise BudgetExceeded("Provider request is not valid JSON") from error
    if not isinstance(request, dict) or request.get("model") != model:
        raise BudgetExceeded("Request model differs from the approved model")
    if request.get("stream") is not True:
        raise BudgetExceeded("Streaming is required for CW-105 measurement")
    limits = [request[key] for key in ("max_tokens", "max_completion_tokens") if key in request]
    if len(limits) != 1 or type(limits[0]) is not int or not 0 < limits[0] <= max_output_tokens:
        raise BudgetExceeded("Request lacks an unambiguous approved output cap")
    if type(request.get("n", 1)) is not int or request.get("n", 1) != 1:
        raise BudgetExceeded("Exactly one completion is allowed")
