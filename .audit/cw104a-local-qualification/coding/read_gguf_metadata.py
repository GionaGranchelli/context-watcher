import struct,json,math,collections
from pathlib import Path
sizes={0:1,1:1,2:2,3:2,4:4,5:4,6:4,7:1,10:8,11:8,12:8};formats={0:'B',1:'b',2:'H',3:'h',4:'I',5:'i',6:'f',7:'?',10:'Q',11:'q',12:'d'}
results=[]
for p in sorted(Path('/home/machinist/Development/Strata/Strata-data/models/IQ2_XS').glob('*.gguf')):
 with p.open('rb') as f:
  def num(fmt):return struct.unpack('<'+fmt,f.read(struct.calcsize('<'+fmt)))[0]
  def string():return f.read(num('Q')).decode('utf-8')
  def value(t,keep=False):
   if t==8:
    n=num('Q')
    if keep:return f.read(n).decode('utf-8')
    f.seek(n,1);return None
   if t==9:
    typ=num('I');n=num('Q')
    if typ in sizes:f.seek(sizes[typ]*n,1)
    else:
     for _ in range(n):value(typ)
    return None
   if t in formats:return num(formats[t])
   raise RuntimeError('Unknown GGUF metadata type '+str(t))
  if f.read(4)!=b'GGUF':raise RuntimeError('Not GGUF')
  version=num('I');nt=num('Q');nk=num('Q');metadata={}
  for _ in range(nk):
   key=string();typ=num('I');keep=key in ['general.name','general.architecture','general.parameter_count','general.file_type','split.no','split.count','split.tensors.count'];v=value(typ,keep)
   if keep:metadata[key]=v
  params=0;types=collections.Counter()
  for _ in range(nt):
   name=string();nd=num('I');dims=[num('Q') for _ in range(nd)];typ=num('I');offset=num('Q');params+=math.prod(dims);types[str(typ)]+=1
  results.append({'source_path':str(p),'file_bytes':p.stat().st_size,'gguf_version':version,'tensor_count':nt,'stored_tensor_elements':params,'quantization_type_id_tensor_counts':dict(types),'metadata':metadata,'metadata_header_end_offset':f.tell()})
print(json.dumps({'scope':'Read-only source GGUF metadata; loaded-artifact equivalence and full weight hashes unverified','shards':results,'total_stored_tensor_elements':sum(x['stored_tensor_elements'] for x in results)},indent=2))
