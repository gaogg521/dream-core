"""Create a NEW offline QA profile; never mutate the source database.

Copy renderer login/config separately if needed, then launch a DEV client with
DREAM_E2E_TEST=1 and DREAM_E2E_USER_DATA_DIR=<output>/profile. This deliberately
models a historical pre-limit database, without weakening immutable API rules.
"""
import argparse
import json
import sqlite3
import time
from pathlib import Path


def create_profile(source, output, provider_id, model, fixture_url):
    source = Path(source).resolve(strict=True)
    output = Path(output).resolve()
    if output.exists() or output == source or output in source.parents:
        raise ValueError('Output must be a new directory distinct from the source')
    # mkdir without exist_ok also rejects a concurrent creator before any write.
    output.mkdir(parents=True)
    data = output / 'profile' / '1one'
    data.mkdir(parents=True)
    workspace = output / 'workspace'
    workspace.mkdir()
    destination = data / 'one-backend.db'
    with sqlite3.connect(source.as_uri() + '?mode=ro', uri=True) as original:
        with sqlite3.connect(destination) as copy:
            original.backup(copy)
    now = int(time.time() * 1000)
    local = [f'qa_history_local_{i:03}' for i in range(100)]
    company = [f'qa_history_company_{i:03}' for i in range(51)]
    conversation = 'qa_hist_151'
    with sqlite3.connect(destination) as db:
        # Only controlled fixtures may execute in the disposable copy.
        db.execute('UPDATE mcp_servers SET enabled=0')
        if any(column[1] == 'enabled' for column in db.execute('PRAGMA table_info(cron_jobs)')):
            db.execute('UPDATE cron_jobs SET enabled=0')
        for kind, ids in [('local', local), ('company', company)]:
            for i, connector in enumerate(ids):
                original = json.dumps({'teamRegistryId': f'qa_history_registry_{i}'}) if kind == 'company' else None
                db.execute(
                    'INSERT INTO mcp_servers (id,user_id,name,enabled,transport_type,transport_config,original_json,builtin,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)',
                    (connector, 'system_default_user', connector, 1, 'http', json.dumps({'url': fixture_url}), original, 0, now, now),
                )
        extra = {
            'workspace': str(workspace), 'custom_workspace': True, 'skills': [],
            'mcp_server_ids': local + company, 'selected_mcp_server_ids': local + company,
            'mcp_servers': local + company, 'qa_history_marker': 'preserve-this-marker',
        }
        db.execute(
            'INSERT INTO conversations (id,user_id,name,type,extra,model,status,created_at,updated_at) VALUES (?,?,?,?,?,?,?,?,?)',
            (conversation, 'system_default_user', 'QA historical 151 MCPs: preserve 100 local', 'dream', json.dumps(extra),
             json.dumps({'provider_id': provider_id, 'model': model}), 'finished', now, now),
        )
        for i, position in enumerate(['right', 'left']):
            text = ['QA_HISTORY_USER_MESSAGE_PRESERVED', 'QA_HISTORY_ASSISTANT_MESSAGE_PRESERVED'][i]
            db.execute(
                'INSERT INTO messages (id,conversation_id,msg_id,type,content,position,status,created_at) VALUES (?,?,?,?,?,?,?,?)',
                (f'qa_hist_msg_{i}', conversation, f'qa_hist_wire_{i}', 'text', json.dumps({'content': text}), position, 'finish', now + i),
            )
        db.commit()
        if db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise RuntimeError('QA database integrity check failed')
    manifest = {'conversation': conversation, 'local': local, 'company': company, 'workspace': str(workspace)}
    (output / 'fixture.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
    return {'profile': str(output / 'profile'), 'conversation': conversation, 'local': len(local), 'company': len(company), 'messages': 2}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-db', required=True)
    parser.add_argument('--output', required=True, help='New disposable directory; existing paths are refused')
    parser.add_argument('--provider-id', required=True, help='Existing provider id in the copied database')
    parser.add_argument('--model', required=True)
    parser.add_argument('--fixture-url', default='http://127.0.0.1:29527/open-mcp')
    args = parser.parse_args()
    print(json.dumps(create_profile(args.source_db, args.output, args.provider_id, args.model, args.fixture_url)))


if __name__ == '__main__':
    main()
