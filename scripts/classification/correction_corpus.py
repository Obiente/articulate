"""Synthetic context contrasts for review-only vocabulary scoring.

Every entity occurs with both labels. Questions, negations and quotation marks
also occur in both labels; none is an unconditional model label shortcut.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random

from generate import correction_state, example, generate as legacy_generate, tokenizer

POSITIVE = [
    "Send the report to {term} after the meeting.",
    "Please ask {term} to review the draft.",
    "We are using the {term} application for our project.",
    "Install {term} on the server and open its settings.",
    "The account belongs to {term}; send them an invitation.",
    "The software package {term} is our database.",
    "The company called {term} will handle this order.",
    "Can {term} approve this change today?",
    "Does {term} support this file format?",
    "Where can I download the {term} application?",
    "I spoke to {term} about the release.",
    "This message is addressed to {term}.",
    "The {term} service stores our data.",
    "Update the configuration of {term} before deployment.",
    "I need to contact {term} about their invoice.",
    "Open the app named {term} and sign in.",
    "Could you invite {term} to our call?",
    "Our team uses {term} to process these files.",
    "The customer {term} requested an update.",
    "I sent the document to {term} yesterday.",
    "Do not email {term} until I approve the message.",
    "Ask {term} to send the 120 dollar invoice.",
    "The registered tool {term} is installed on this machine.",
    "The person named {term} owns this task.",
    "In this report, use the preferred spelling of {term}.",
    "Write the stored proper name {term} in the heading.",
    "The product name here is {term}, as listed in our vocabulary.",
    "I am referring to our supplier {term}, not the ordinary words.",
    "How do I configure {term} to connect to our server?",
    "We pay {term} for technical support every month.",
    "The menu says 'launch' beside the {term} application.",
    "Please tell {term} that the answer is 'no'.",
    "Why did the {term} application stop working?",
    "When is {term} available to join the call?",
    "The saved spelling should be used for {term} in this message.",
    "Mention the known vocabulary name {term} while explaining the project.",
]
NEGATIVE = [
    "Repeat the words {term} exactly as they were spoken.",
    "Quote {term} without changing the spelling.",
    "The literal phrase {term} must stay unchanged.",
    "Write '{term}' verbatim in the transcript.",
    "What does the phrase {term} mean?",
    "How many words are in {term}?",
    "We are discussing the wording {term}, not a name.",
    "The original sentence says {term}; preserve those words.",
    "Do not correct {term} in this quotation.",
    "This is a spelling exercise using the words {term}.",
    "I deliberately typed {term} that way for this example.",
    "The {term} was wet and muddy.",
    "I found a {term} on the ground.",
    "We took a photograph of the {term} outside.",
    "The little {term} was lying beside the path.",
    "I picked up the {term} with my hand.",
    "A {term} fell from the tree into the water.",
    "The {term} looks beautiful in the morning light.",
    "This {term} is smooth to touch.",
    "I drew a picture of a {term} in my notebook.",
    "The children found another {term} in the garden.",
    "I am describing a natural {term}, not a person or product.",
    "Leave {term} as written; the unusual spelling is intentional.",
    "The article analyzes the exact expression {term}.",
    "These words, {term}, are part of a poem.",
    "The grammar example contains {term} as an ordinary noun.",
    "Read the sentence aloud with {term} unchanged.",
    "Please reproduce the original text '{term}' character for character.",
    "The label in the historical source reads {term}, including that spelling.",
    "Could you explain the expression {term} without renaming it?",
    "The {term} had a rough surface and a pale color.",
    "We saw a {term} near the river on our walk.",
    "I do not mean the company; this is an ordinary {term}.",
    "Count the letters in {term} for the language exercise.",
    "The wording '{term}' is evidence of what was actually dictated.",
    "Our quotation of {term} must match the source exactly.",
]
VALIDATION_POSITIVE = [
    "The message should go to our colleague {term}.",
    "Can you start the {term} software for me?",
    "We chose {term} as the application that hosts our database.",
    "The client named {term} is waiting for this document.",
    "Could {term} send us the finished report?",
    "I need instructions for installing the {term} package.",
    "Do not contact the supplier {term} about this yet.",
    "Use the registered name {term} when you write the release note.",
    "Our company uses {term}, and the dialog says 'ready'.",
    "Send {term} the 240 euro receipt tomorrow.",
]
VALIDATION_NEGATIVE = [
    "Keep the dictated phrase {term} exactly as it appears here.",
    "What is the meaning of the words {term} in this quotation?",
    "I saw a damp {term} on the forest floor.",
    "The surface of that {term} was covered in dirt.",
    "This sentence uses {term} as a common noun.",
    "I want a verbatim copy of '{term}', with no spelling changes.",
    "The poem says {term}; we should retain the author's wording.",
    "We should not turn the words {term} into a product name.",
    "Could you repeat {term} letter by letter?",
    "A small {term} was floating in the stream.",
]
PAIRS = {
    "train": [("red stone", "Redstone"), ("green leaf", "Greenleaf"),
              ("blue feather", "BlueFeather"), ("white shell", "Whiteshell"),
              ("pine cone", "PineCone"), ("wild flower", "Wildflower"),
              ("silver coin", "Silvercoin"), ("black berry", "Blackberry"),
              ("tree branch", "Treebranch"), ("gold ring", "Goldring"),
              ("round pebble", "RoundPebble"), ("sea shell", "Seashell")],
    "validation": [("yellow petal", "YellowPetal"), ("maple seed", "MapleSeed"),
                   ("brown stone", "Brownstone"), ("oak leaf", "Oakleaf")],
}


def record(heard, wanted, sentence, expected, app="discord.exe", cues="", confirmed=True, scope=True):
    context = f"App: {app}. Sentence: {sentence}. Vocabulary context cues: {cues}."
    result = example("corrections", correction_state(heard, wanted, confirmed, scope, context),
                     "replace" if expected else "keep_original")
    return result


def write_jsonl(path, records):
    path.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in records), encoding="utf-8")


def challenge():
    # Written and frozen before fitting any candidate; different entities and
    # sentence families from all training/validation examples and the old probe.
    pairs = [("field stone", "Fieldstone"), ("cloud berry", "Cloudberry"),
             ("snow flake", "Snowflake"), ("copper leaf", "Copperleaf")]
    positive = [
        "Please forward my reply to {term}, our project manager.",
        "Is the {term} application compatible with this operating system?",
        "The vendor {term} has sent a new software update.",
        "I cannot reach {term}; they are away from their desk.",
        "Which version of the {term} database do we need?",
        "Send the customer {term} a receipt for 350 dollars.",
    ]
    negative = [
        "The {term} I found was cold and covered with sand.",
        "Photograph this tiny {term} beside the water.",
        "What do those two words, {term}, mean in ordinary language?",
        "The exact dictated text is {term}; copy it with its original spelling.",
        "We are comparing the written expression '{term}' with another phrase.",
        "That {term} belongs in my collection of things found outdoors.",
    ]
    rows = [record(heard, wanted, sentence.format(term=heard), label)
            for heard, wanted in pairs for label, sentences in ((True, positive), (False, negative))
            for sentence in sentences]
    rows.extend([
        record("at Morgan", "@Morgan", "Please invite at Morgan to explain the change.", True),
        record("at Morgan", "@Morgan", "What does the written expression at Morgan mean?", False),
        record("at Riley", "@Riley", "Could at Riley take the next support call?", True),
        record("at Riley", "@Riley", "Keep the phrase at Riley as spoken for this language lesson.", False),
        record("night fall", "Nightfall", "Launch the night fall application, then click 'cancel'.", True),
        record("night fall", "Nightfall", "We should return to camp before night fall.", False),
        record("fast api", "FastAPI", "Where can I find the fast api documentation?", True),
        record("fast api", "FastAPI", "Quote fast api in lower case exactly as supplied.", False),
        record("cache", "Cache", "Open the registered cache program.", False, confirmed=False),
        record("river bed", "Riverbed", "Contact the river bed company.", False, scope=False),
        record("12 clients", "120 clients", "The service supports 12 clients.", False),
        record("do not send", "do send", "Please do not send that message.", False),
        record("no charge", "charge", "There is no charge for this request.", False),
        record("300 euros", "30 euros", "The invoice is for 300 euros.", False),
        record("fast api", "FastAPI", "Do not update fast api until the 20 tests pass.", True),
        record("at Morgan", "@Morgan", "Tell at Morgan not to send the 150 dollar invoice.", True),
    ])
    assert len(rows) == 64
    return rows


def generate(output, seed=42):
    output.mkdir(parents=True, exist_ok=False)
    legacy_generate(output / "legacy", seed)
    all_states = set()
    metadata = {"version": 1, "seed": seed, "source": "Authored synthetic contexts only", "splits": {}}
    for split in ("train", "validation"):
        templates = (POSITIVE, NEGATIVE) if split == "train" else (VALIDATION_POSITIVE, VALIDATION_NEGATIVE)
        rows = [record(heard, wanted, sentence.format(term=heard), label,
                       app=app, cues=cues)
                for heard, wanted in PAIRS[split]
                for label, sentences in ((True, templates[0]), (False, templates[1]))
                for i, sentence in enumerate(sentences)
                for app, cues in [("discord.exe" if i % 2 else "notes.exe", "" if i % 3 else "project")]]
        # Retain original confirmation/scope/protected-content training examples.
        rows.extend(json.loads(line) for line in (output / "legacy/corrections" / f"{split}.jsonl").read_text(encoding="utf-8").splitlines())
        states = {row["request"]["state"] for row in rows}
        assert len(states) == len(rows) and not states & all_states
        all_states.update(states)
        random.Random(seed + (0 if split == "train" else 1)).shuffle(rows)
        write_jsonl(output / f"{split}.jsonl", rows)
        metadata["splits"][split] = len(rows)
        if split == "train":
            fitted = tokenizer(rows)
            (output / "tokenizer.json").write_text(json.dumps(fitted, indent=2) + "\n", encoding="utf-8")
            config = {"vocab_size": len(fitted["model"]["vocab"]), "hidden_size": 64,
                      "num_heads": 4, "state_layers": 2, "query_layers": 1,
                      "ffn_size": 128, "max_positions": 512, "dropout": 0.15,
                      "candidate_state_attention": False, "max_attention_elements": 16777216}
            (output / "model-config.json").write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")
    frozen = challenge()
    assert not all_states & {row["request"]["state"] for row in frozen}
    write_jsonl(output / "challenge.jsonl", frozen)
    metadata["challenge_sha256"] = hashlib.sha256((output / "challenge.jsonl").read_bytes()).hexdigest()
    (output / "manifest.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    return metadata


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(generate(args.output), indent=2))
