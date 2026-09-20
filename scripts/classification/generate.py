"""Deterministic, synthetic-only Articulate classifier corpus. No user data is read."""
import argparse, collections, hashlib, json, random, re
from pathlib import Path

GOAL = "Capture final decisions, assigned actions, and important facts."
CONTRACTS = {
    "corrections": {"id":"correction", "text":"Should this proposed dictionary correction replace the original text?", "candidates":[
        {"id":"keep_original","description":"Keep the original text unchanged; the replacement is unsafe, unconfirmed, ambiguous, or out of scope."},
        {"id":"replace","description":"Apply this confirmed dictionary replacement in its matching context without changing meaning."}]},
    "shortcuts": {"id":"shortcut_intent", "text":"How should this utterance be handled for the registered voice shortcut?", "candidates":[
        {"id":"literal","description":"Ordinary speech discussing or quoting a shortcut; preserve the utterance unchanged."},
        {"id":"command","description":"A complete explicit invocation of the registered text shortcut at the start of the utterance."},
        {"id":"incomplete","description":"An unfinished trigger; wait for more speech and preserve the text meanwhile."},
        {"id":"unknown","description":"No registered matching shortcut or usable context; preserve the utterance unchanged."}]}}
NOTE_TEMPLATES = {
"decision":["We agreed to use {topic} for the next release.","The final decision is to keep {topic} unchanged.","Everyone approved {topic} as our shared approach.","We have chosen {topic} and closed that discussion.","The team decided to postpone {topic} until review.","Our agreed choice is to retain {topic} for now.","It is settled: {topic} will remain in the plan.","We reached a final agreement to ship {topic} first.","After the vote, we confirmed {topic} as the next priority.","The approved plan now includes {topic}; no further decision is pending."],
"action":["Casey will review {topic} by Friday.","Jordan is assigned to test {topic} tomorrow.","I will write the checklist for {topic} before lunch.","Casey owns the follow-up for {topic} this week.","Jordan will send the revised {topic} plan by Thursday.","I am responsible for documenting {topic} before our next call.","The next step belongs to Casey: verify {topic} today.","Jordan committed to checking {topic} before the deadline.","Please record this assignment: Casey will validate {topic} on Monday.","Jordan has taken ownership of the {topic} review and will report on Tuesday."],
"key_fact":["The {topic} package is currently 240 megabytes.","Our tests found that {topic} requires a microphone permission.","The main blocker for {topic} is the missing device driver.","The {topic} test failed on two CPU-only machines.","We measured a three-second delay when loading {topic}.","The current limit for {topic} is four simultaneous speakers.","One important constraint: {topic} must work without a network connection.","The reported result for {topic} is a five-percent error rate.","The evidence shows {topic} uses twice the memory on older devices.","For planning purposes, {topic} cannot start until the dependency is installed."],
"background":["Maybe we could try {topic}, but nothing is agreed.","I wonder whether {topic} might look nicer in blue.","Thanks for joining the conversation about {topic}.","Someone mentioned {topic} earlier; I have no update.","I am just repeating the name {topic} to check the microphone.","We have not decided anything about {topic} yet.","Would anyone perhaps like to discuss {topic} another day?", "There is only a tentative suggestion about {topic}, not a commitment.","The idea of changing {topic} is speculation and has no owner.","Before we start, how was your weekend? We can talk about {topic} later."]}
TOPICS={"train":["microphone setup","call notes","vocabulary editor","keyboard shortcut"],"validation":["session export","model download","audio routing","history search"],"test":["speaker labels","template expansion","onboarding checklist","update prompt"]}
FAMILIES={"train":range(6),"validation":range(6,8),"test":range(8,10)}
PAIRS={"train":[("at Casey","@Casey"),("articulate","Articulate"),("check in","check-in"),("qwen","Qwen")],"validation":[("at Jordan","@Jordan"),("follow up","follow-up"),("github","GitHub"),("rust lang","Rustlang")],"test":[("at Taylor","@Taylor"),("sign in","sign-in"),("web view","WebView"),("type script","TypeScript")]}
TRIGGERS={"train":["signature","follow-up","update","address"],"validation":["greeting","schedule","agenda","thanks"],"test":["summary","welcome","handoff","reminder"]}


def correction_state(original, proposed, confirmed, scope, context):
    return f"Original: {original}\nProposed: {proposed}\nConfirmed dictionary entry: {'yes' if confirmed else 'no'}\nApp scope matches: {'yes' if scope else 'no'}\nContext: {context}"


def shortcut_state(trigger, utterance, focused=True):
    return f"Registered trigger: {trigger}\nUtterance: {utterance}\nText field focused: {'yes' if focused else 'no'}"


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2)+"\n",encoding="utf-8")


def example(task, state, label):
    q=CONTRACTS[task]
    return {"request":{"state":state,"questions":[q]},"targets":[{"kind":"hard","value":[c['id'] for c in q['candidates']].index(label)}]}


def tokenizer(examples):
    texts=[]
    for e in examples:
        texts.append(e['request']['state'])
        for q in e['request']['questions']:
            texts.append(q['text']); texts.extend(c['description'] for c in q['candidates'])
    counts=collections.Counter(t for text in texts for t in re.findall(r"\w+|[^\w\s]+",text.lower()))
    vocab={"[PAD]":0,"[UNK]":1}
    for token,_ in sorted(counts.items(),key=lambda p:(-p[1],p[0])): vocab.setdefault(token,len(vocab))
    return {"version":"1.0","truncation":None,"padding":None,"added_tokens":[{"id":i,"content":s,"single_word":False,"lstrip":False,"rstrip":False,"normalized":False,"special":True} for s,i in list(vocab.items())[:2]],"normalizer":{"type":"Lowercase"},"pre_tokenizer":{"type":"Whitespace"},"post_processor":None,"decoder":None,"model":{"type":"WordLevel","vocab":vocab,"unk_token":"[UNK]"}}


def generate(output, seed=42):
    output.mkdir(parents=True,exist_ok=False)
    rng=random.Random(seed)
    manifest={"version":1,"seed":seed,"source":"authored synthetic scenarios only","splits":{},"limitations":"Family-disjoint synthetic templates are a development benchmark, not proof of real conversation quality."}
    for task in ('notes','corrections','shortcuts'): (output/task).mkdir()
    seen={task:set() for task in ('notes','corrections','shortcuts')}
    for split, indices in FAMILIES.items():
        notes=[]; corrections=[]; shortcuts=[]; provenance=[]
        for f in indices:
            for variant,topic in enumerate(TOPICS[split]):
                group=f'{split}-family-{f}-scenario-{variant}'
                segments=[]; labels=[]
                order=list(NOTE_TEMPLATES); rng.shuffle(order)
                for i,kind in enumerate(order):
                    segments.append({"id":f's{i}',"start_ms":i*7000,"end_ms":i*7000+6000,"speaker":["Casey","Jordan"][i%2],"text":NOTE_TEMPLATES[kind][f].format(topic=topic)})
                    labels.append({"segment_id":f's{i}',"kind":kind})
                notes.append({"transcript":{"id":group,"title":f'{topic} review',"goal":GOAL,"segments":segments},"labels":labels})
                original,proposed=PAIRS[split][variant]
                contexts=[f'An edit in the {topic} discussion.',f'The user is writing a {topic} message.',f'A draft about {topic} is focused.',f'The active note describes {topic}.',f'A saved rule is being checked during {topic}.',f'The text comes from a {topic} follow-up.',f'This candidate occurs while explaining {topic}.',f'The surrounding sentence concerns {topic}.',f'The speaker is sending an update on {topic}.',f'The current task is a review of {topic}.']
                context=contexts[f]
                records=[(original,proposed,True,True,'replace','confirmed'),(original,proposed,False,True,'keep_original','unconfirmed'),(original,proposed,True,False,'keep_original','scope'),(f'do not send {original}',f'do send {proposed}',True,True,'keep_original','negation'),(f'{original} costs 120',f'{proposed} costs 12',True,True,'keep_original','number'),(f'maybe {original}',f'certainly {proposed}',False,True,'keep_original','meaning')]
                for before,after,confirmed,scope,label,reason in records:
                    e=example('corrections',correction_state(before,after,confirmed,scope,context),label);corrections.append(e);provenance.append({'task':'corrections','state_sha256':hashlib.sha256(e['request']['state'].encode()).hexdigest(),'family':f'correction-{f}','reason':reason})
                positive = ["Use the saved spelling for {original} in this {topic} message.", "The product called {original} belongs in the {topic} note.", "Write {original} using its registered spelling for {topic}.", "Mention the known name {original} while explaining {topic}.", "This {topic} update refers to the vocabulary term {original}.", "Apply the preferred name {original} in the {topic} draft.", "The named tool {original} is part of our {topic} workflow.", "For the {topic} summary, the registered name is {original}.", "In this {topic} report, refer to {original} as the stored proper name.", "Our {topic} document should use the recognized spelling of {original}."]
                literal = ["Quote the exact words '{original}' in the {topic} example.", "The literal text '{original}' must remain as typed for {topic}.", "Discuss the phrase '{original}' without changing its spelling in {topic}.", "For {topic}, show precisely what was spoken: '{original}'.", "Preserve '{original}' verbatim in this {topic} quotation.", "The {topic} sample deliberately uses the uncorrected spelling '{original}'.", "This {topic} passage analyzes the exact wording '{original}'.", "Keep the quoted transcript '{original}' unchanged in {topic}.", "The {topic} report must reproduce the original phrase '{original}' character for character.", "We are documenting how '{original}' was written in the {topic} source, not renaming it."]
                app = {'train':'code.exe','validation':'chat.exe','test':'notes.exe'}[split]
                for sentence, label in [(positive[f], 'replace'), (literal[f], 'keep_original')]:
                    sentence = sentence.format(original=original, topic=topic)
                    local_context = f'App: {app}. Sentence: {sentence}. Vocabulary context cues: {topic}.'
                    e=example('corrections',correction_state(original,proposed,True,True,local_context),label)
                    corrections.append(e)
                    provenance.append({'task':'corrections','state_sha256':hashlib.sha256(e['request']['state'].encode()).hexdigest(),'family':f'correction-context-{f}','reason':'sentence-context'})
                trigger='bang '+TRIGGERS[split][variant]
                payload=contexts[f]
                literal=[f'Please explain the phrase {trigger}.',f'I said the words {trigger} earlier.',f'Write about {trigger} as an example.',f'The shortcut named {trigger} is useful.',f'Can you describe {trigger} to me?',f'That sentence contains {trigger}.',f'The documentation mentions {trigger}.',f'I am discussing {trigger}, not invoking it.',f'Our glossary should explain what {trigger} means.',f'Tell Jordan the command is called {trigger}.'][f]
                cases=[(trigger+' '+payload,True,'command'),(literal,True,'literal'),('bang',True,'incomplete'),('bang unregistered '+payload,True,'unknown'),(trigger+' '+payload,False,'unknown')]
                for utterance,focused,label in cases:
                    # Context keeps repeated unfinished trigger examples uniquely
                    # grouped, while preserving a stable deployed input contract.
                    state=shortcut_state(trigger,utterance,focused)
                    e=example('shortcuts',state,label)
                    if not any(x['request']['state']==state for x in shortcuts):shortcuts.append(e)
        rng.shuffle(corrections);rng.shuffle(shortcuts);rng.shuffle(notes)
        manifest['splits'][split]={"notes_transcripts":len(notes),"correction_examples":len(corrections),"shortcut_examples":len(shortcuts),"template_families":list(indices)}
        for task,data in [('notes',notes),('corrections',corrections),('shortcuts',shortcuts)]:
            states={e['request']['state'].strip().lower() for e in data} if task!='notes' else {s['text'].strip().lower() for e in data for s in e['transcript']['segments']}
            assert not states&seen[task],f'{task} split overlap';seen[task].update(states)
            if task=='notes':write(output/task/(split+'.json'),data)
            else:
                (output/task/(split+'.jsonl')).write_text(''.join(json.dumps(e,ensure_ascii=False)+'\n' for e in data),encoding='utf-8')
                write(output/task/(split+'-requests.json'),[e['request'] for e in data])
                if split=='train':
                    tok=tokenizer(data);write(output/task/'tokenizer.json',tok)
                    write(output/task/'model-config.json',{"vocab_size":len(tok['model']['vocab']),"hidden_size":32,"num_heads":4,"state_layers":1,"query_layers":1,"ffn_size":64,"max_positions":512,"dropout":0.1,"candidate_state_attention":False,"max_attention_elements":16777216})
        write(output/f'{split}-provenance.json',provenance)
    write(output/'contracts.json',CONTRACTS);write(output/'manifest.json',manifest)
    return manifest

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--seed',type=int,default=42);args=p.parse_args();print(json.dumps(generate(args.output,args.seed),indent=2))
