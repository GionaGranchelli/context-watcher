"""Offline CW-122B admission contract. This module cannot dispatch inference."""
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import stat
import sys
from urllib.parse import urlsplit

from analyze import require
from oracle import ROOT, digest

# Reuse the existing exclusive, durable authorization-consumption marker.
sys.path.insert(0, str(ROOT.parent / 'cw104b'))
from smoke import claim_authorization
sys.path.pop(0)

FIELDS = {'campaign_id','authorization_id','approved_by','approval_reference','issued_at','expires_at','provider','model','endpoint',
          'campaign_sha256','runner_freeze_sha256','max_runs','max_provider_requests','max_seconds','spend_ceiling_usd',
          'accept_reservation_overrun','credential_file','evidence_dir','credentials_handling'}


def validate(document, selection, now=None):
    require(isinstance(document,dict) and set(document)==FIELDS, 'Missing or unknown authorization fields')
    require(document['campaign_id']=='CW-122B', 'Previous campaign authorization is invalid')
    for key in ['authorization_id','approved_by','approval_reference','provider','model']:
        require(isinstance(document[key],str) and 0<len(document[key].strip())<=256 and 'UNAPPROVED' not in document[key], 'Explicit authorization identity required: '+key)
    try:
        issued,expires = (datetime.fromisoformat(document[k].replace('Z','+00:00')) for k in ['issued_at','expires_at'])
        now = now or datetime.now(timezone.utc)
        current = issued.tzinfo is not None and expires.tzinfo is not None and issued<=now<expires and 0<(expires-issued).total_seconds()<=86400
    except (TypeError,ValueError,AttributeError):
        current=False
    require(current,'Authorization expired, future or invalid')
    endpoint = urlsplit(document['endpoint'])
    require(endpoint.scheme in ['http','https'] and endpoint.hostname and not endpoint.username and not endpoint.password and not endpoint.query and not endpoint.fragment, 'Invalid endpoint')
    for key in ['provider','model','endpoint','evidence_dir','credential_file']:
        require(document[key]==selection[key], 'Authorization selection mismatch: '+key)
    require(document['campaign_sha256']==digest(ROOT/'campaign.json') and document['runner_freeze_sha256']==digest(ROOT/'freeze.json'), 'Campaign/runner freeze mismatch')
    require(type(document['max_runs']) is int and document['max_runs']==12, 'Exactly six paired M1/M2 runs must be scoped')
    for key,limit in [('max_provider_requests',360),('max_seconds',7200)]:
        require(type(document[key]) is int and 0<document[key]<=limit, 'Invalid admission limit: '+key)
    budget=document['spend_ceiling_usd']
    require(type(budget) in [int,float] and 0<budget<1_000_000, 'Explicit finite positive spend ceiling required')
    require(document['accept_reservation_overrun'] is True, 'Reservations are not hard caps; fresh explicit overrun-risk acceptance required')
    require(document['credentials_handling']=='private_file_outside_agent_environment', 'Credential isolation contract missing')
    credential=Path(document['credential_file']); evidence=Path(document['evidence_dir'])
    require(credential.is_absolute() and not credential.is_symlink() and credential.is_file() and stat.S_IMODE(credential.stat().st_mode)==0o600,'Private regular credential file must be 0600')
    require(evidence.is_absolute() and not evidence.is_symlink() and evidence.is_dir() and stat.S_IMODE(evidence.stat().st_mode)==0o700,'Private evidence directory must be 0700')
    require(not credential.resolve().is_relative_to(evidence.resolve()) and not evidence.resolve().is_relative_to(ROOT.parent.parent) and not credential.resolve().is_relative_to(ROOT.parent.parent),'Credentials/evidence must be separate and outside the checkout')
    record={k:v for k,v in document.items() if k not in ['credential_file','evidence_dir']}
    record.update(record_valid=True,consumed=False,document_sha256=hashlib.sha256(json.dumps(document,sort_keys=True).encode()).hexdigest())
    return record


def claim(document, selection, now=None):
    record=validate(document,selection,now)
    claim_authorization(Path(document['evidence_dir']),record)
    return record
