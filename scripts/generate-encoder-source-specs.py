#!/usr/bin/env python3
"""Regenerate fixed, bounded scalar specs from the pinned Nmkoder extract.

Manually adjudicated choices/ranges below remain reviewed application policy.
Generated Rust never accepts user-provided option names.
"""
import json,math,re,pathlib
root=pathlib.Path(__file__).resolve().parent.parent
j=json.loads((root/'tests/fixtures/nmkoder-encoder-catalog.json').read_text())
LEVELS=['auto','1','1.0','1b','1.1','1.2','1.3','2','2.0','2.1','2.2','3','3.0','3.1','3.2','4','4.0','4.1','4.2','5','5.0','5.1','5.2','6','6.0','6.1','6.2']
choices={
 'tune-content':['default','screen','film'], 'deadline':['good','best','realtime'],
 'error-resilient':['0','default','partitions','default+partitions'],
 'dist-metric':['psnr','qm-psnr'], 'sb-size':['dynamic','64','128'],
 'ctu':['16','32','64'], 'min-cu-size':['8','16','32'],
 'level-idc':['0','1','2','2.1','3','3.1','4','4.1','5','5.1','5.2','6','6.1','6.2','10','20','21','30','31','40','41','50','51','52','60','61','62'],
 'me':['dia','hex','umh','star','sea','full'],
 'tf_level':['-1','0','4'], 'lookahead_level':['-1','0','1','2','3','auto'],
 'rc':['constqp','vbr','cbr'], 'multipass':['disabled','qres','fullres'],
 'b_ref_mode':['-1','disabled','each','middle'], 'coder':['default','auto','cabac','cavlc'],
 'split_encode_mode':['auto','forced','2','3','4','disabled'], 'tier':['main','high'], 'max-tx-size':['32','64'],
}
manual={
 'rc-lookahead':('Whole',0,250), 'g':('Whole',0,2147483647), 'bf':('Whole',-1,16), 'refs':('Whole',1,16),
 'deblock':('PairWhole',-6,6), 'psy-rd':('Decimal',0,5), 'aq-strength':('Decimal',0,3),
 'qcomp':('Decimal',0,1), 'ipratio':('Decimal',1,10), 'pbratio':('Decimal',1,10),
 'qp_cb_offset':('Whole',-12,12), 'qp_cr_offset':('Whole',-12,12),
 'cbqpoffs':('Whole',-12,12), 'crqpoffs':('Whole',-12,12),
 'qpmin':('Whole',0,81), 'qpmax':('Whole',0,81), 'qpstep':('Whole',0,81),
 'nr':('Whole',0,100000), 'vbv-maxrate':('Whole',0,2147483647), 'vbv-bufsize':('Whole',0,2147483647),
 'crf-max':('Decimal',0,51), 'ref':('Whole',1,8),
 'static-thresh':('Whole',0,2147483647), 'max-intra-rate':('Whole',0,2147483647),
 'max-inter-rate':('Whole',0,2147483647), 'min-cr':('Whole',0,2147483647),
 'denoise-noise-level':('Whole',0,50), 'film-grain':('Whole',0,50),
 'qmin':('Whole',-1,51), 'qmax':('Whole',-1,51), 'init_qpI':('Whole',-1,51), 'init_qpP':('Whole',-1,51), 'init_qpB':('Whole',-1,51),
 'min-gf-interval':('Whole',-1,24), 'highbitdepth':('Whole',0,1), 'scm':('Whole',0,3),
}
owned={'Libx265':{'master-display','max-cll'},'LibSvtAv1':{'film-grain','film-grain-denoise'},'LibAomAv1':{'denoise-noise-level','enable-dnl-denoising'}}
fnnames={'Libx264':'library_x264','Libx265':'library_x265','LibVpx':'library_vpx','LibSvtAv1':'library_svt','LibAomAv1':'library_aom',}
def q(s):return json.dumps(s,ensure_ascii=False)
def build(c):
 name=pathlib.Path(c['sourceFile']).stem; output=[]
 for r in c['options']:
  n=r['option']
  if n in owned.get(name,set()):continue
  cs=choices.get(n)
  if n=='profile:v':cs=['baseline','main','high','high10','high422','high444p']
  if n=='profile':cs=['main','main10','rext','mv']
  if n=='level':cs=(['-1']+LEVELS[1:] if name=='LibVpx' else LEVELS)
  if n=='tune':
   cs=['psnr','ssim'] if name=='LibVpx' else (['psnr','ssim','iq','ssimulacra2','vmaf_with_preprocessing','vmaf_without_preprocessing','vmaf','vmaf_neg','butteraugli','vmaf_saliency_map'] if name=='LibAomAv1' else ['hq','uhq','ll','ull','lossless'] if name=='H265Nvenc' else ['hq','ll','ull','lossless'] if name=='H264Nvenc' else None)
  if n=='me' and name=='Libx264':cs=['dia','hex','umh','esa','tesa']
  if n=='direct':cs=['spatial','temporal','auto','none']
  if n=='b-pyramid':cs=['none','strict','normal']
  if n=='partitions':cs=['p8x8','b8x8','i8x8','i4x4','p4x4','all','none']
  if cs:kind='ChoiceList' if n=='partitions' else 'Choice';lo=hi=0
  elif n=='psy-rd' and name=='Libx264':kind,lo,hi='PairDecimal',0,10
  elif n=='aq-strength' and name.startswith('H'):kind,lo,hi='Whole',1,15
  elif n=='ref' and name=='Libx264':kind,lo,hi='Whole',1,16
  elif n in manual:kind,lo,hi=manual[n]
  elif r['rangeBounds'] and len(r['rangeBounds'])==2:
   bounds=list(map(float,r['rangeBounds']));kind='Decimal' if any('.' in x for x in r['rangeBounds']) else 'Whole';lo=math.floor(bounds[0]);hi=math.ceil(bounds[1])
  elif '0 or 1' in r['syntaxDescription'] or r['syntaxDescription'].startswith('0-1'):kind,lo,hi='Whole',0,1
  else:raise Exception((name,n,r['syntaxDescription']))
  if name=='LibSvtAv1' and n=='tune':kind,lo,hi='Whole',0,5;cs=None
  ex=r.get('listedValuesDescription','').splitlines()[0].split('|')[0] if r.get('listedValuesDescription') else ''
  if kind.startswith('Pair'):ex=ex.replace(',',':')
  if cs:
   if ex not in cs:ex=cs[0]
  else:
   try:
    vals=list(map(float,ex.split(':')))
    if len(vals)!=(2 if kind.startswith('Pair') else 1) or not all(lo<=v<=hi for v in vals):raise ValueError()
   except:ex=(f'{lo}:{lo}' if kind.startswith('Pair') else str(lo))
  flag=n if name=='Libx265' else '-'+n
  desc=r['syntaxDescription']+'\n\n'+r['description']
  output.append(f'        Spec {{ name: {q(n)}, label: {q(n)}, flag: {q(flag)}, min: {lo}, max: {hi}, kind: ValueKind::{kind}, choices: &['+', '.join(q(x) for x in (cs or []))+f'], group: {q(r["category"])}, description: {q(desc)}, example: {q(ex)} }},')
 return f'pub(super) fn {fnnames[name]}() -> Vec<Spec> {{\n    vec![\n'+'\n'.join(output)+'\n    ]\n}\n'
text='//! Typed scalar controls from the pinned fork source catalogs.\n//! Source text is preserved; path/HDR/grain resource controls remain owned by typed jobs.\nuse super::{Spec, ValueKind};\n\n'
for c in j['catalogs']:
 if c['sourceWrapper']=='ffmpeg' and pathlib.Path(c['sourceFile']).stem in fnnames:text+=build(c)+'\n'
for source,fnname in [('Libx264','native_x264'),('Libx265','native_x265'),('LibSvtAv1','native_svt'),('LibAomAv1','native_aom')]:
 c=next(c for c in j['catalogs'] if pathlib.Path(c['sourceFile']).stem==source)
 libfn=fnnames[source]
 text+=f'pub(super) fn {fnname}() -> Vec<Spec> {{\n    {libfn}().into_iter().map(|mut spec| {{\n        spec.flag = match spec.name {{\n'
 for r in c['options']:
  n=r['option']
  if n in owned.get(source,set()):continue
  text+=f'            {q(n)} => {q("--"+n)},\n'
 text+='            _ => unreachable!("source-generated scalar"),\n        };\n        spec\n    }).collect()\n}\n'
text+='pub(super) fn source_text(encoder: media_core::VideoEncoder, name: &str) -> Option<(&\'static str, &\'static str)> {\n    use media_core::VideoEncoder as V;\n    match (encoder, name) {\n'
for c in j['catalogs']:
 name=pathlib.Path(c['sourceFile']).stem
 mapping={'X264':'V::X264','X265':'V::X265Standalone','SvtAv1':'V::SvtAv1 | V::SvtAv1FiveFish | V::SvtAv1Hdr','AomAv1':'V::AomAv1','Vpx':'V::VpxStandalone','Libx265':'V::X265','LibVpx':'V::Vp9'}
 if name not in mapping:continue
 for r in c['options']:
  text+=f'        ({mapping[name]}, {q(r["option"])}) => Some(({q(r["category"])}, {q(r["syntaxDescription"]+chr(10)+chr(10)+r["description"])})),\n'
text+='        _ => None,\n    }\n}\n'
(root/'crates/media-runtime/src/jobs/parameters/source_specs.rs').write_text(text)
print('Generated typed source_specs.rs')
