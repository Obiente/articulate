"""Run the external Rust Assort trainer and report conservative held-out behavior."""
import argparse, hashlib, json, re, subprocess, sys, time
from pathlib import Path
from generate import generate, CONTRACTS

NEGATIONS={'not','never','no','without','cannot',"can't","don't","isn't","wasn't"}
NUMBER_WORDS=set('zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty thirty forty fifty sixty seventy eighty ninety hundred thousand million billion first second third fourth fifth half quarter'.split())

def protected_tokens(text):
    words=re.findall(r"[\w']+",text.lower())
    return ([w for w in words if w in NEGATIONS],
            [w for w in words if w in NUMBER_WORDS],
            re.findall(r'[+-]?\d+(?:[.,]\d+)*(?:%)?',text),
            re.findall(r'[$€£]',text))

def correction_allowed(state):
    fields=dict(line.split(': ',1) for line in state.splitlines() if ': ' in line)
    return (fields.get('Confirmed dictionary entry')=='yes' and fields.get('App scope matches')=='yes'
        and protected_tokens(fields.get('Original',''))==protected_tokens(fields.get('Proposed','')))

def shortcut_allowed(state):
    fields=dict(line.split(': ',1) for line in state.splitlines() if ': ' in line)
    trigger=fields.get('Registered trigger','').strip().lower()
    utterance=fields.get('Utterance','').strip().lower()
    return bool(trigger and fields.get('Text field focused')=='yes' and (utterance==trigger or utterance.startswith(trigger+' ')))

def run(exe,args,log):
    started=time.monotonic()
    with log.open('w',encoding='utf-8') as stream:
        result=subprocess.run([str(exe),*map(str,args)],stdout=stream,stderr=subprocess.STDOUT,check=False)
    if result.returncode:
        raise RuntimeError(f'Assort failed ({result.returncode}); see {log}')
    return time.monotonic()-started

def read_examples(path):
    return [json.loads(line) for line in path.read_text(encoding='utf-8-sig').splitlines() if line.strip()]

def evaluate(exe,task,data,model,output):
    examples=read_examples(data/'test.jsonl'); predictions=[]
    for offset in range(0,len(examples),8):
        batch=examples[offset:offset+8]
        result=subprocess.run([str(exe),'infer','--checkpoint',str(model/'checkpoint'),'--tokenizer',str(model/'tokenizer.json'),'--input','-'],input=json.dumps([e['request'] for e in batch]),capture_output=True,text=True,encoding='utf-8',check=True)
        predictions.extend(json.loads(result.stdout))
    labels=[c['id'] for c in examples[0]['request']['questions'][0]['candidates']]
    confusion=[[0]*len(labels) for _ in labels];correct=0;false_positive=0;negatives=0;accepted=0;accepted_fp=0;retained=0;details=[]
    action='replace' if task=='corrections' else 'command'
    for e,response in zip(examples,predictions):
        a=response['answers'][0]; true=labels[e['targets'][0]['value']]; pred=a['selected_id']
        probs=a['distribution']['probabilities'];confidence=probs[a['candidate_ids'].index(pred)]
        confusion[labels.index(true)][labels.index(pred)]+=1;correct+=pred==true
        if true!=action:negatives+=1;false_positive+=pred==action
        gate=correction_allowed(e['request']['state']) if task=='corrections' else shortcut_allowed(e['request']['state'])
        apply=pred==action and confidence>=0.90 and gate
        accepted+=apply;accepted_fp+=apply and true!=action;retained+=not apply
        fields=dict(line.split(': ',1) for line in e['request']['state'].splitlines() if ': ' in line)
        original=fields['Original'] if task=='corrections' else fields['Utterance']
        # Training reports never expand or edit live text. Preserve exact input
        # on abstention; model output is a typed decision, not generated prose.
        result_text=fields['Proposed'] if apply and task=='corrections' else original
        if not apply: assert result_text==original
        details.append({'true':true,'predicted':pred,'confidence':confidence,'deterministic_gate':gate,'action_accepted':apply,'original':original,'result':result_text})
    positives=len(examples)-negatives
    report={'examples':len(examples),'labels':labels,'accuracy':correct/len(examples),'confusion':confusion,'raw_action_false_positives':false_positive,'negative_examples':negatives,'positive_examples':positives,'raw_action_false_positive_rate':false_positive/negatives if negatives else 0,'fixed_threshold':0.90,'gated_actions':accepted,'gated_action_false_positives':accepted_fp,'gated_action_recall':(accepted-accepted_fp)/positives if positives else 0,'gated_action_precision':(accepted-accepted_fp)/accepted if accepted else None,'exact_preservation_abstentions':retained,'source':'untouched family-disjoint synthetic test; no real-world quality claim'}
    (output/'test-report.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    (output/'test-predictions.json').write_text(json.dumps(details,indent=2)+'\n',encoding='utf-8')
    return report

def hashes(model):
    return {p.relative_to(model).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(model.rglob('*')) if p.is_file() and (p.parent.name=='checkpoint' or p.name in ('tokenizer.json','inference-limits.json'))}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--assort',type=Path,required=True,help='Path to the built external Assort CLI');p.add_argument('--output',type=Path,required=True,help='New local run directory');p.add_argument('--seed',type=int,default=42);p.add_argument('--epochs',type=int,default=12);p.add_argument('--task',choices=['all','notes','corrections','shortcuts'],default='all');p.add_argument('--evaluate-existing',action='store_true',help='Evaluate existing task directories beside data-v1, without retraining');args=p.parse_args()
    exe=args.assort.resolve();root=args.output.resolve()
    if not exe.is_file():p.error('--assort must point to an existing CLI executable')
    if args.epochs<1:p.error('--epochs must be positive')
    if args.evaluate_existing:
        data=root/'data-v1'
    else:
        root.mkdir(parents=True,exist_ok=False);data=root/'data';generate(data,args.seed)
    summary={'seed':args.seed,'epochs':args.epochs,'source':'synthetic development baseline','tasks':{}}
    for task in (('notes','corrections','shortcuts') if args.task == 'all' else (args.task,)):
        model=root/(task+'-v1' if args.evaluate_existing else task)
        if not args.evaluate_existing:
            common=['--output',model,'--epochs',args.epochs,'--batch-size',4 if task=='notes' else 8,'--seed',args.seed]
            if task=='notes': train_args=['train-transcripts','--train',data/task/'train.json','--validation',data/task/'validation.json','--test',data/task/'test.json',*common]
            else:train_args=['train','--train',data/task/'train.jsonl','--validation',data/task/'validation.jsonl','--tokenizer',data/task/'tokenizer.json','--config',data/task/'model-config.json',*common]
            elapsed=run(exe,train_args,root/(task+'.log'))
            if task == 'corrections':
                # Written only after this recipe trained the exact task. Do not
                # attach task metadata to arbitrary pre-existing checkpoints.
                contract = {'version': 1, 'task': task, 'question': CONTRACTS[task]}
                (model/'checkpoint'/'articulate-task.json').write_text(json.dumps(contract, indent=2)+'\n', encoding='utf-8')
                # Preserve the trainer's token lengths while restricting the
                # app's request cardinality to this two-candidate task.
                limits = {'max_batch_size':1,'max_questions':1,'max_candidates':2,'max_state_tokens':512,'max_question_tokens':128,'max_candidate_tokens':128,'max_padded_tokens':262144}
                (model/'inference-limits.json').write_text(json.dumps(limits, indent=2)+'\n', encoding='utf-8')
        else:elapsed=None
        if task=='notes':
            report=json.loads((model/'transcript-test-report.json').read_text(encoding='utf-8'))
            bg=report['category_order'].index('background');negative=sum(report['confusion'][bg]);fp=negative-report['confusion'][bg][bg]
            report['background_false_positive_rate']=fp/negative if negative else 0
        else:report=evaluate(exe,task,data/task,model,model)
        summary['tasks'][task]={'training_seconds':elapsed,'test':report,'sha256':hashes(model)}
        print(task,json.dumps({'accuracy':report.get('accuracy',report.get('category_accuracy')),'training_seconds':elapsed}),flush=True)
    (root/'evaluation-summary.json').write_text(json.dumps(summary,indent=2)+'\n',encoding='utf-8')
    print('All artifacts are local. These models remain experimental; inspect held-out errors before any application or release.')

if __name__=='__main__':main()
