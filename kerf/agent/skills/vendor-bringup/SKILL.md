# vendor-bringup

Bring a new vendor into the registry by recording one supervised run.

## When to use

A quote or order was requested for a vendor with no playbook (or a
playbook whose canary is red and whose steps no longer match the site).

## Procedure

1. `load_playbook` — confirm what exists. If a playbook exists, your run
   is a REPAIR: keep its step ids stable wherever the flow still matches.
2. Open the vendor site inside the session (the allowlist already scopes
   you to the manifest's domains). Walk the flow for the given intent
   exactly as a careful human would, narrating each action.
3. At each action, capture: the semantic selector you actually used
   (role/label/text before css), a screenshot, and — before anything
   money-adjacent — the extraction + assertion that would have to hold
   (price, quantity, material label).
4. Never proceed past a payment prompt, CAPTCHA, or login wall:
   `request_takeover`.
5. When the flow completes (a price for quote capability; a staged cart
   for order capability — then `request_confirmation`), distill the trace
   into a Playbook (see `@kerf/core` playbook format):
   - one step per action, semantic selectors, `from_intent` value refs —
     never literals copied from this run's intent;
   - `money_adjacent: true` + assertions on quantity, price, and any
     add-to-cart/config-apply step (the validator rejects bare money
     steps);
   - `on_fail`: "abort" for money steps, "agent_repair" for navigation.
6. `propose_playbook_patch` with the playbook JSON, the DOM fixture, and a
   summary of what changed and why.

## Quality bar

A playbook is done when a second run of the same intent executes Tier-1
clean — no agent turns — and the canary spec can point at it.
