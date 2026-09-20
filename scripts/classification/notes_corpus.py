"""Authored English meeting-note examples, never real conversation data.

Template families and topic names are disjoint between training, validation,
and test. The test split must not be used to select weights or hyperparameters.
These labels describe explicit commitments, facts and informal conversation;
they do not establish accuracy on real speech or unseen languages.
"""
import argparse
import hashlib
import json
import random
from pathlib import Path

GOAL = "Capture final decisions, assigned actions, and important facts."
TRAIN = {
    "decision": [
        "We decided to keep {topic} in the next release.",
        "The team agreed to remove {topic} from the plan.",
        "Our final decision is to postpone {topic} until next month.",
        "We approved {topic} and the discussion is closed.",
        "After reviewing both options, we chose {topic}.",
        "It is settled: we are going ahead with {topic}.",
        "Everyone agreed that {topic} stays as it is.",
        "The approved plan is to launch {topic} on Monday.",
        "We reached a final agreement to cancel {topic}.",
        "The vote is complete and {topic} was approved.",
        "Our choice is confirmed: {topic} will be included.",
        "We have made the decision not to release {topic} yet.",
        "We agreed not to change {topic} this week.",
        "There is no further vote needed; {topic} is approved.",
        "The team rejected {topic} after the review.",
        "We have settled on {topic} instead of the old option.",
        "I accept the proposal to keep {topic}; that is our final choice.",
        "We are committed to the plan for {topic} now.",
        "For the record, we decided that {topic} comes first.",
        "The final outcome of the meeting is to retain {topic}.",
        "That settles the issue: {topic} will not be shipped.",
        "We all support the agreed approach to {topic}.",
        "The proposal for {topic} has been accepted.",
        "We confirmed the decision to stop work on {topic}.",
        "No, we decided to leave {topic} unchanged.",
        "Okay, agreed. We will proceed with {topic}.",
        "The committee has voted against {topic}.",
        "We will use {topic}; that is the decision we reached today.",
        "We selected {topic} as the replacement for the current system.",
        "Approval for {topic} is final and recorded.",
    ],
    "action": [
        "Casey will review {topic} by Friday.",
        "Jordan is assigned to test {topic} tomorrow.",
        "I will write the checklist for {topic} before lunch.",
        "Casey owns the follow-up for {topic} this week.",
        "Jordan will send the revised {topic} plan by Thursday.",
        "I am responsible for documenting {topic} before the next call.",
        "The next step belongs to Casey: verify {topic} today.",
        "Jordan committed to checking {topic} before the deadline.",
        "Please record this assignment: Casey will validate {topic} on Monday.",
        "Jordan has taken ownership of the {topic} review and will report on Tuesday.",
        "I will send the results for {topic} to everyone after this meeting.",
        "I can take that task. I will check {topic} this afternoon.",
        "We assigned Casey to investigate the failure in {topic}.",
        "Jordan agreed to prepare the {topic} report by tomorrow morning.",
        "I am taking the action to update {topic} before Friday.",
        "Casey will not publish {topic} until the tests pass; Casey owns that check.",
        "Jordan is responsible for confirming the cost of {topic} next week.",
        "My follow-up is to ask the supplier about {topic} today.",
        "I promise to deliver the {topic} draft by noon.",
        "The assigned owner for testing {topic} is Casey, due Monday.",
        "Jordan accepted the task of fixing {topic} before release.",
        "I will follow up on {topic} and send you an update tomorrow.",
        "Casey must complete the {topic} review by the agreed deadline.",
        "Put me down to prepare {topic} for the next meeting.",
        "I have accepted responsibility for the {topic} checklist.",
        "Jordan will contact the team about {topic} after lunch.",
        "The task is assigned: Casey will document {topic} on Wednesday.",
        "I am going to test {topic} this evening and share the result.",
        "Casey has committed to sending the {topic} notes today.",
        "I will check {topic}, not the other feature, before the deadline.",
    ],
    "key_fact": [
        "The {topic} package is currently 240 megabytes.",
        "Our tests found that {topic} requires a microphone permission.",
        "The main blocker for {topic} is the missing device driver.",
        "The {topic} test failed on two CPU-only machines.",
        "We measured a three-second delay when loading {topic}.",
        "The current limit for {topic} is four simultaneous speakers.",
        "One important constraint: {topic} must work without a network connection.",
        "The reported result for {topic} is a five-percent error rate.",
        "The evidence shows {topic} uses twice the memory on older devices.",
        "For planning purposes, {topic} cannot start until the dependency is installed.",
        "The supplier confirmed that {topic} costs 600 dollars per year.",
        "The release deadline for {topic} is the end of June.",
        "The current version of {topic} does not support mobile devices.",
        "We cannot deploy {topic} because the security review is incomplete.",
        "The latest measurement shows {topic} takes twelve seconds to start.",
        "There are three unresolved defects in {topic}.",
        "The budget available for {topic} is 800 euros.",
        "No customer data was lost during the {topic} outage.",
        "The tests for {topic} passed on every supported device.",
        "The contract for {topic} expires on the first of September.",
        "Our capacity for {topic} is limited to ten requests per minute.",
        "The {topic} service has been unavailable since nine this morning.",
        "The audit found that {topic} is missing two required checks.",
        "The most recent survey showed that half the users need {topic}.",
        "The important dependency for {topic} is already installed.",
        "According to the report, {topic} increased memory use by twenty percent.",
        "The team does not have permission to access {topic} yet.",
        "The scheduled maintenance for {topic} starts at midnight.",
        "We have verified that {topic} works offline.",
        "A critical requirement for {topic} is support for Windows.",
    ],
    "background": [
        "Maybe we could try {topic}, but nothing is agreed.",
        "I wonder whether {topic} might look nicer in blue.",
        "Thanks for joining the conversation about {topic}.",
        "Someone mentioned {topic} earlier; I have no update.",
        "I am just repeating the name {topic} to check the microphone.",
        "We have not decided anything about {topic} yet.",
        "Would anyone perhaps like to discuss {topic} another day?",
        "There is only a tentative suggestion about {topic}, not a commitment.",
        "The idea of changing {topic} is speculation and has no owner.",
        "Before we start, how was your weekend? We can talk about {topic} later.",
        "I have not agreed to review {topic}; please do not assign it to me.",
        "Nobody has committed to delivering {topic} on Friday.",
        "We might approve {topic} if the results look good, but not today.",
        "If I had time, I would test {topic}, but I cannot promise anything.",
        "Could someone maybe send a report about {topic}? We have no volunteer yet.",
        "We are still discussing whether to keep {topic}; the vote is not complete.",
        "That was an example of a decision, not an actual approval of {topic}.",
        "I was joking when I said I would finish {topic} tomorrow.",
        "The suggestion about {topic} is only a question at this point.",
        "I do not know the deadline for {topic}; I would only be guessing.",
        "Can you repeat what you said about {topic}?",
        "I have no confirmed figures for {topic}, so let us avoid assumptions.",
        "We should consider {topic}, perhaps, if we get a chance.",
        "There is no final choice about {topic} and no assigned action.",
        "I cannot take ownership of {topic}; someone else would need to volunteer.",
        "The decision about {topic} is still pending.",
        "The phrase 'I will review {topic}' was a sample sentence, not my commitment.",
        "This is not a task assignment for {topic}; we are brainstorming.",
        "I am not sure if {topic} is ready; that is just speculation.",
        "Does anyone remember where we put the notes about {topic}?",
    ],
}
# Additional contrastive families keep words such as "accepted", "confirmed",
# "not" and "will" from acting as category labels by themselves.
TRAIN["decision"].extend([
    "We accepted the revised proposal to continue {topic}.",
    "The proposed approach to {topic} is approved by the whole team.",
    "After the discussion we agreed: stop changing {topic}.",
    "We have agreed on the plan to replace {topic}.",
    "Our choice to release {topic} has been confirmed by everyone.",
    "We chose not to include {topic}, and that outcome is final.",
    "The team has accepted keeping {topic} in the current version.",
    "Okay, let us record the decision: we are dropping {topic}.",
    "Yes, the vote is final and we will retain {topic}.",
    "For this release, we approved the option without {topic}.",
    "We finished the discussion and reached agreement on {topic}.",
    "We are no longer considering alternatives; {topic} is our confirmed choice.",
    "That is approved. Our new plan includes {topic}.",
    "We will continue using {topic}; everyone has agreed to that choice.",
    "The agreed position is to leave {topic} out of the launch.",
    "We rejected the proposed change to {topic} and closed the vote.",
])
TRAIN["action"].extend([
    "Casey accepted responsibility for reviewing {topic} before Wednesday.",
    "I have agreed to deliver {topic} before the next review.",
    "Jordan confirmed that they will write the {topic} documentation tomorrow.",
    "The action is mine: I will check whether {topic} passes the tests.",
    "Casey has volunteered to investigate {topic} this week.",
    "I will make sure the {topic} plan reaches the team today.",
    "Jordan is the assigned owner of the {topic} report, which is due tonight.",
    "We agreed that Casey will verify {topic} and report the numbers.",
    "Put the {topic} follow-up against my name; I will do it tomorrow.",
    "I will review {topic} on Monday and send the results on Tuesday.",
    "Casey is taking the next step and will prepare {topic} for testing.",
    "I accepted that task. I will finish the {topic} check this week.",
    "Jordan has agreed to own the work on {topic} until release.",
    "I will confirm the {topic} budget with the supplier after the meeting.",
    "Casey will investigate why {topic} is blocked before Friday.",
    "The team assigned me to collect the {topic} measurements tomorrow.",
])
TRAIN["key_fact"].extend([
    "The confirmed budget for {topic} is 200 dollars.",
    "The approved cost limit for {topic} remains 300 euros.",
    "Our test results show that {topic} needs 150 megabytes of memory.",
    "A missing driver is blocking {topic} on the test devices.",
    "The latest report confirms that {topic} is not available on mobile.",
    "The team confirmed that {topic} currently fails three checks.",
    "We found a permission problem that prevents {topic} from starting.",
    "The supplier has confirmed the delivery date for {topic}: July first.",
    "The {topic} budget was approved at 400 euros last month.",
    "No, the measurements show that {topic} needs eight seconds to finish.",
    "Our agreed deadline for {topic} is the last day of May.",
    "A security requirement prevents us from deploying {topic} now.",
    "Testing has confirmed that {topic} uses two gigabytes on this device.",
    "The limit we have agreed with the supplier for {topic} is twelve users.",
    "A missing approval is the current blocker for the {topic} release.",
    "According to our test evidence, {topic} failed without a network connection.",
])
TRAIN["background"].extend([
    "Nobody accepted the assignment to work on {topic}.",
    "No one agreed to own {topic} and we are still looking for a volunteer.",
    "The proposed {topic} plan has not been accepted or approved.",
    "We did not decide to release {topic}; it is still under discussion.",
    "Perhaps we will go with {topic}, but no choice has been confirmed.",
    "Thank you for explaining {topic}. Sorry, I missed the first part.",
    "Okay, thanks. Could you repeat the point about {topic}?",
    "I am only asking whether {topic} might be useful.",
    "Someone could investigate {topic}, but I am not assigning that work.",
    "The vote on {topic} has not happened and approval is not final.",
    "I never promised to deliver {topic} this week.",
    "We cannot call {topic} an agreed plan yet.",
    "That is just a possible deadline for {topic}, not a confirmed date.",
    "I was reading a fictional example of someone accepting the {topic} task.",
    "Nobody has volunteered to check {topic} by tomorrow.",
    "This proposed change to {topic} is not a decision; we need more discussion.",
])
VALIDATION = {
    "decision": [
        "Our agreed decision today is to include {topic} in the launch.",
        "We have now accepted the proposed plan for {topic}.",
        "After discussion, the team confirmed that {topic} should be removed.",
        "The vote has settled the question: keep {topic}.",
        "It is final. We decided not to proceed with {topic}.",
        "Approval is recorded for the new approach to {topic}.",
        "We agreed that the existing {topic} will remain unchanged.",
        "Our final choice after the review was {topic}.",
    ],
    "action": [
        "Morgan will complete the {topic} checklist and send it on Friday.",
        "The follow-up for {topic} has been assigned to Riley for tomorrow.",
        "I will take responsibility for checking {topic} before release.",
        "Riley agreed to contact the supplier about {topic} today.",
        "Morgan owns the task of preparing {topic} for Monday.",
        "I have committed to writing the {topic} report by next week.",
        "Record an action for Riley to verify {topic} after this call.",
        "I will deliver the revised {topic} plan before the deadline.",
    ],
    "key_fact": [
        "Our latest tests confirmed that {topic} requires 400 megabytes of memory.",
        "A missing permission is currently blocking {topic}.",
        "The agreed budget limit for {topic} is 900 dollars.",
        "According to the measurements, {topic} takes four seconds to load.",
        "The release of {topic} depends on a security check that has not finished.",
        "The supplier contract for {topic} ends in December.",
        "We verified that {topic} does not work on older devices.",
        "The current error rate for {topic} is seven percent.",
    ],
    "background": [
        "Perhaps we will choose {topic}, but the final decision has not been made.",
        "I might review {topic} if I can find the time; do not count that as a promise.",
        "No one has accepted the task of testing {topic} yet.",
        "We are only considering {topic}, and approval is still pending.",
        "I was quoting a possible action about {topic}, not volunteering.",
        "I cannot confirm any numbers for {topic}; I was only guessing.",
        "Thank you for the explanation of {topic}. Can you repeat the last part?",
        "There is no commitment to ship {topic}; this is still a suggestion.",
    ],
}
TEST = {
    "decision": [
        "We reached agreement today: {topic} is the option we will use.",
        "The team has approved removing {topic} from the next release.",
        "Following the vote, we decided to keep {topic} for another month.",
        "The discussion is closed with a final choice to cancel {topic}.",
        "We have confirmed that {topic} will not change in this release.",
        "That proposal has been accepted; {topic} is now part of the agreed plan.",
        "Our decision after the review is not to launch {topic} yet.",
        "Everyone agreed to proceed with {topic}, so we can close this issue.",
    ],
    "action": [
        "Taylor has accepted the job of reviewing {topic} and will finish on Thursday.",
        "I will prepare an update on {topic} for our next call.",
        "The task of checking {topic} belongs to Avery, due Friday morning.",
        "Taylor committed to contacting the supplier about {topic} tomorrow.",
        "I will take the follow-up on {topic} and report back next week.",
        "Avery is responsible for delivering the {topic} checklist before noon.",
        "We have assigned Taylor to test {topic} before Monday's release.",
        "I will not send {topic} until I have checked it; I will complete that check today.",
    ],
    "key_fact": [
        "The results confirm that {topic} currently consumes 500 megabytes.",
        "An incomplete security review is the blocker for {topic}.",
        "We measured a six-second startup time for {topic} on the test machine.",
        "The {topic} contract ends in March, which limits our available time.",
        "There are five open defects preventing the release of {topic}.",
        "The test report confirms that {topic} does not support offline use.",
        "The current cost of {topic} is 700 euros each year.",
        "A required driver is missing, so {topic} cannot run on those devices.",
    ],
    "background": [
        "We have not approved {topic}; it remains a possible option for discussion.",
        "I said I might check {topic}, not that I would definitely take the task.",
        "If someone volunteers we could review {topic}, but there is no owner today.",
        "That sentence about delivering {topic} was an example, not a commitment.",
        "It is too early to make a decision on {topic}; no agreement was reached.",
        "I do not have verified costs for {topic}, so those numbers would be guesses.",
        "Could you explain the idea of {topic} again? I missed what you said.",
        "We are brainstorming changes to {topic}, with nothing assigned or settled.",
    ],
}
TOPICS = {
    "train": ["microphone setup", "call notes", "vocabulary editor", "keyboard shortcut",
              "project schedule", "customer portal", "office network", "warehouse system"],
    "validation": ["session export", "model download", "audio routing", "history search"],
    "test": ["speaker labels", "template expansion", "onboarding checklist", "update prompt"],
}


def generate(output, seed=42):
    output.mkdir(parents=True, exist_ok=False)
    rng = random.Random(seed)
    seen = set()
    manifest = {"version": 1, "seed": seed, "source": "authored synthetic English only",
                "split_unit": "disjoint sentence template families and topic names", "splits": {}}
    for split, templates in (("train", TRAIN), ("validation", VALIDATION), ("test", TEST)):
        records = [(kind, text.format(topic=topic)) for kind, texts in templates.items()
                   for text in texts for topic in TOPICS[split]]
        normalized = {text.lower().strip() for _, text in records}
        assert len(normalized) == len(records) and not normalized & seen
        seen.update(normalized)
        rng.shuffle(records)
        corpus = []
        # Avoid the former invariant that every conversation has exactly one
        # passage of each category. Real windows may be all small talk or actions.
        for offset in range(0, len(records), 4):
            segments, labels = [], []
            for index, (kind, text) in enumerate(records[offset:offset + 4]):
                identity = f"s{index}"
                segments.append({"id": identity, "start_ms": index * 7000,
                                 "end_ms": index * 7000 + 6000,
                                 "speaker": ["Speaker A", "Speaker B"][index % 2], "text": text})
                labels.append({"segment_id": identity, "kind": kind})
            corpus.append({"transcript": {"id": f"{split}-{offset // 4}", "title": "Synthetic meeting",
                                          "goal": GOAL, "segments": segments}, "labels": labels})
        data = (json.dumps(corpus, ensure_ascii=False, indent=2) + "\n").encode()
        (output / f"{split}.json").write_bytes(data)
        manifest["splits"][split] = {"transcripts": len(corpus), "segments": len(records),
                                    "families_per_category": len(templates["decision"]),
                                    "sha256": hashlib.sha256(data).hexdigest()}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--seed", type=int, default=42)
    arguments = parser.parse_args()
    print(json.dumps(generate(arguments.output, arguments.seed), indent=2))
