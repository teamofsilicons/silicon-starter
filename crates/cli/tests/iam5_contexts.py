#!/usr/bin/env python3
"""Hermetic IAM5 CLI contracts. Pass the built starter binary; never contacts production."""
import hashlib, http.server, json, os, pathlib, subprocess, sys, tempfile, threading, unittest, uuid
BINARY = str(pathlib.Path(sys.argv.pop(1)).resolve())
class ContextTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='starter-iam5-')
        self.root = pathlib.Path(self.temp.name)
        self.env = {k:v for k,v in os.environ.items() if not k.startswith(('STARTER_', 'SILICON_', 'GIT_'))}
        self.env.update(SILICON_HOME=str(self.root), HOME=str(self.root), GIT_AUTHOR_NAME='Test', GIT_AUTHOR_EMAIL='test@example.invalid', GIT_COMMITTER_NAME='Test', GIT_COMMITTER_EMAIL='test@example.invalid', STARTER_NO_DAEMON='1')
        self.requests=[];self.sessions={};self.logins={};self.fail_login=False;self.fail_complete=False;self.fail_publish=False;self.terms_changed=False;self.status_override={};self.approval_id=str(uuid.uuid4());self.request_id=str(uuid.uuid4())
        self.delay_list=False;self.list_started=threading.Event();self.list_release=threading.Event();self.permission_declined=False;self.testing_world='testing:'+str(uuid.uuid4())
        test=self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self,*_): pass
            def do_GET(self): self.handle_request()
            def do_POST(self): self.handle_request()
            def handle_request(self):
                body=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))) or b'null')
                test.requests.append((self.command,self.path,{k.lower():v for k,v in self.headers.items()},body))
                status=200
                if self.path=='/auth/cli':
                    key=self.headers.get('Idempotency-Key');assert key
                    if test.fail_login: test.fail_login=False;status=503;value={'error':'unavailable'}
                    else:
                        if key not in test.logins:
                            actor={'type':'carbon','public_id':'c:alice'} if body['slt']=='oac_home' else {'type':'silicon','public_id':'si:worker'}
                            org='personal' if body['slt']=='oac_home' else 'tos'
                            value={'authenticated':True,'session_id':'secret-'+str(uuid.uuid4()),'context_id':str(uuid.uuid4()),'actor':actor,'org_id':org,'world':'production','world_fingerprint':'production:1'}
                            if body['slt']=='si:tester':value.update(actor={'type':'silicon','public_id':'si:tester'},org_id=body['org_id'],world=test.testing_world,world_fingerprint=test.testing_world+':generation:1')
                            test.logins[key]=value;test.sessions[value['session_id']]=value
                        value=test.logins[key]
                else:
                    saved=test.sessions.get(self.headers.get('X-Starter-Session'))
                    if saved and self.headers.get('X-Starter-Context')!=saved['context_id']:status=401;value={'error':'context header missing'}
                    elif self.path=='/auth/cli/status':value={**saved,**test.status_override};value.pop('session_id',None)
                    elif self.path.startswith('/api/v1/briefcase/'):
                        assert saved
                        completed=self.path.endswith('/complete')
                        if completed and test.terms_changed:status=412;value={'error':{'code':'version_mismatch'}}
                        elif completed and test.permission_declined:status=403;value={'error':{'code':'reconsent_required'}}
                        elif completed and test.fail_complete:test.fail_complete=False;status=503;value={'error':'uncertain'}
                        else:value={'request_id':test.request_id,'context_id':saved['context_id'],'completed':completed,'roots':[], 'authorization':{'id':test.approval_id,'app_id':'starter','actor':saved['actor'],'org_id':saved['org_id'],'status':'exchanged' if completed else 'pending','version':1,'state':None,'expires_at':'2099-01-01T00:00:00Z','authorization_url':'https://iam.example/review'}}
                    elif self.path=='/api/v1/blocks' and self.command=='POST':
                        if test.fail_publish:test.fail_publish=False;status=403;value={'error':{'code':'reconsent_required'}}
                        else:value={'id':body['id']}
                    elif self.path.endswith('/publish'):
                        if test.fail_publish:test.fail_publish=False;status=403;value={'error':{'code':'reconsent_required','details':{'feature':'briefcase'}}}
                        else:value={'version':body['version'],'commit':body['commit']}
                    elif self.path=='/api/v1/starters':
                        if test.delay_list and self.command=='GET':
                            test.list_started.set();assert test.list_release.wait(10)
                        value=[] if self.command=='GET' else {'id':body['id']}
                    else: value={}
                data=json.dumps(value).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
        self.api=f'http://127.0.0.1:{self.server.server_port}'
    def tearDown(self):self.server.shutdown();self.server.server_close();self.thread.join();self.temp.cleanup()
    def run_cli(self,*args,profile='work',world='production',cwd=None,stdin=None,ok=True):
        result=subprocess.run([BINARY,'--api',self.api,'--profile',profile,'--world',world,*args],cwd=cwd or self.root,env=self.env,text=True,input=stdin,capture_output=True)
        self.assertEqual(result.returncode==0,ok,result.stderr)
        return result
    def store(self,profile='work',world='production'):
        digest=hashlib.sha256(f'{self.api}\n{world}'.encode()).hexdigest()
        return self.root/'.starter/profiles'/profile/digest
    def login(self,profile='work',token='oac_work'):return self.run_cli('login',token,profile=profile)
    def test_independent_profiles_legacy_rejection_and_org_mismatch(self):
        self.login();self.login('home','oac_home')
        work=json.loads((self.store()/'session.json').read_text());home=json.loads((self.store('home')/'session.json').read_text())
        self.assertNotEqual(work['context_id'],home['context_id']);self.assertEqual(home['actor']['type'],'carbon')
        self.run_cli('list');self.assertEqual(self.requests[-1][2]['x-starter-session'],work['session_id'])
        count=len(self.requests);self.run_cli('--org','other','list',ok=False);self.assertEqual(len(self.requests),count)
        (self.root/'.starter/session').write_text('legacy-secret')
        self.run_cli('list',profile='default');self.assertNotIn('x-starter-session',self.requests[-1][2])
        self.run_cli('login','si:worker',world='testing:'+str(uuid.uuid4()),ok=False)
        self.assertEqual((self.store()/'session.json').stat().st_mode&0o777,0o600)
    def test_uncertain_login_reuses_key_and_body(self):
        self.fail_login=True;self.run_cli('login','oac_work',ok=False)
        self.run_cli('login','oac_other',ok=False);self.assertEqual(len(self.requests),1)
        self.run_cli('login','--recover')
        self.assertEqual(self.requests[0][2]['idempotency-key'],self.requests[1][2]['idempotency-key']);self.assertEqual(self.requests[0][3],self.requests[1][3])
        self.assertFalse((self.store()/'login-pending.json').exists())
    def test_testing_world_keeps_independent_credentials_and_requires_exact_world(self):
        self.login();production=json.loads((self.store()/'session.json').read_text())
        self.run_cli('--org','test-org','login','si:tester',world=self.testing_world)
        testing=json.loads((self.store(world=self.testing_world)/'session.json').read_text())
        self.assertEqual(testing['org_id'],'test-org');self.assertNotEqual(production['context_id'],testing['context_id'])
        self.run_cli('list',world=self.testing_world);self.assertEqual(self.requests[-1][2]['x-starter-session'],testing['session_id'])
        self.run_cli('list');self.assertEqual(self.requests[-1][2]['x-starter-session'],production['session_id'])
        self.run_cli('--org','test-org','login','si:tester',profile='wrong-world',world='testing:'+str(uuid.uuid4()),ok=False)
    def test_status_rejects_changed_actor_org_or_world_stamp(self):
        self.login()
        for override in [{'org_id':'elsewhere'},{'actor':{'type':'carbon','public_id':'c:bob'}},{'world_fingerprint':'production:changed'}]:
            self.status_override=override;self.run_cli('login','status','--json',ok=False)
    def test_response_from_replaced_login_is_not_applied(self):
        self.login();self.delay_list=True
        job=subprocess.Popen([BINARY,'--api',self.api,'--profile','work','list'],cwd=self.root,env=self.env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            self.assertTrue(self.list_started.wait(5))
            self.login('work','oac_home')
        finally:self.list_release.set()
        stdout,stderr=job.communicate(timeout=10)
        self.assertNotEqual(job.returncode,0);self.assertIn('login changed',stderr);self.assertEqual(stdout,'')
    def test_daemon_uses_each_checkout_origin_and_rejects_replacement_login(self):
        self.login();repo=self.root/'background';repo.mkdir();self.run_cli('new','public','tos.example',cwd=repo)
        binding_path=repo/'.git/starter.json';binding=json.loads(binding_path.read_text());binding.update(mode='download');binding_path.write_text(json.dumps(binding));(self.root/'.starter/registry.json').write_text(json.dumps([{'path':str(repo),'binding':binding}]))
        original=json.loads((self.store()/'session.json').read_text())
        self.login('home','oac_home');self.run_cli('daemon','--once',profile='home')
        self.assertTrue(self.requests[-1][1].endswith('/archive'))
        self.assertEqual(self.requests[-1][2]['x-starter-session'],original['session_id'])
        self.assertEqual(self.requests[-1][2]['x-starter-context'],original['context_id'])
        self.login('work','oac_home');count=len(self.requests)
        result=self.run_cli('daemon','--once',profile='home')
        self.assertEqual(len(self.requests),count);self.assertIn('checkout belongs to another saved login',result.stderr)
    def test_permission_completion_recovery_does_not_publish(self):
        self.login();self.run_cli('permission','authorize');self.fail_complete=True
        self.run_cli('permission','complete','--code-file','-',stdin='code-secret\n',ok=False)
        self.run_cli('permission','complete')
        pair=self.requests[-2:];self.assertEqual(pair[0][2]['idempotency-key'],pair[1][2]['idempotency-key']);self.assertEqual(pair[0][3],pair[1][3])
        self.assertNotIn('code-secret',(self.store()/'briefcase-permission.json').read_text())
        self.assertFalse(any(r[1].endswith('/publish') for r in self.requests))
    def test_changed_terms_discard_review_and_use_new_start_key(self):
        self.login();self.run_cli('permission','authorize');first=self.requests[-1][2]['idempotency-key'];self.terms_changed=True
        self.run_cli('permission','complete','--code-file','-',stdin='code',ok=False)
        self.assertFalse((self.store()/'briefcase-permission.json').exists())
        self.run_cli('permission','authorize');self.assertNotEqual(first,self.requests[-1][2]['idempotency-key'])
    def test_declined_permission_clears_code_and_allows_fresh_review(self):
        self.login();self.run_cli('permission','authorize');first=self.requests[-1][2]['idempotency-key'];self.permission_declined=True
        self.run_cli('permission','complete','--code-file','-',stdin='declined-code',ok=False)
        self.assertFalse((self.store()/'briefcase-permission.json').exists())
        self.run_cli('permission','authorize');self.assertNotEqual(first,self.requests[-1][2]['idempotency-key'])
    def test_publication_retains_original_commit_and_context(self):
        self.login();repo=self.root/'repo';repo.mkdir();self.run_cli('new','public','tos.example',cwd=repo)
        self.fail_publish=True;self.run_cli('publish','latest','1.0','--notes','first notes',cwd=repo,ok=False)
        original=self.requests[-1];self.assertTrue((repo/'.git/starter-publication.json').exists())
        (repo/'README.md').write_text('Changed after first attempt\n')
        self.run_cli('commit','later work',cwd=repo)
        self.run_cli('publish','latest','1.0','--notes','first notes',cwd=repo,ok=False)
        self.run_cli('publish','retry',cwd=repo)
        self.assertEqual(original[2]['idempotency-key'],self.requests[-1][2]['idempotency-key']);self.assertEqual(original[3],self.requests[-1][3])
        count=len(self.requests);self.login('work','oac_home');self.run_cli('publish','latest','1.1',cwd=repo,ok=False);self.assertEqual(len(self.requests),count+1)
        self.assertFalse((repo/'.git/starter-publication.json').exists())
    def test_block_publication_retains_bytes_key_and_metadata_and_rejects_replacement_login(self):
        self.login();self.fail_publish=True
        self.run_cli('publish','gene:original','--text','# Original','--description','saved',ok=False)
        original=self.requests[-1];receipt=self.store()/'block-publication.json';self.assertTrue(receipt.exists())
        count=len(self.requests)
        self.run_cli('publish','gene:original','--text','# Changed',ok=False);self.assertEqual(len(self.requests),count)
        self.run_cli('publish','gene:original','--retry')
        self.assertEqual(original[3],self.requests[-1][3]);self.assertEqual(original[2]['idempotency-key'],self.requests[-1][2]['idempotency-key']);self.assertFalse(receipt.exists())
        self.fail_publish=True;self.run_cli('publish','gene:original','--text','# Another',ok=False)
        self.login('work','oac_home');count=len(self.requests)
        self.run_cli('publish','gene:original','--retry',ok=False);self.assertEqual(len(self.requests),count);self.assertTrue(receipt.exists())
    def test_block_local_cancel_does_not_mutate_provider_or_login(self):
        self.login();self.fail_publish=True;self.run_cli('publish','gene:original','--text','# Original',ok=False)
        count=len(self.requests);self.run_cli('publish','gene:original','--cancel');self.assertEqual(len(self.requests),count)
        self.assertFalse((self.store()/'block-publication.json').exists());self.assertTrue((self.store()/'session.json').exists())
if __name__=='__main__':unittest.main()
