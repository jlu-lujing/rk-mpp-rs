import sys

class BW:
    def __init__(s): s.bits=[]
    def put(s,v,n):
        v &= (1<<n)-1
        for i in range(n): s.bits.append((v>>i)&1)
    def align(s,n,ones):
        while len(s.bits)%n: s.bits.append(1 if ones else 0)
    def bytes(s,n):
        out=bytearray(n)
        for i,b in enumerate(s.bits):
            if b: out[i//8] |= 1<<(i%8)
        return bytes(out)

class BR:
    def __init__(s,b): s.b=b; s.p=0
    def bit(s):
        v=(s.b[s.p//8]>>(7-s.p%8))&1; s.p+=1; return v
    def bits(s,n):
        v=0
        for _ in range(n): v=(v<<1)|s.bit()
        return v
    def ue(s):
        z=0
        while s.bit()==0: z+=1
        return (1<<z)-1+(s.bits(z) if z else 0)
    def se(s):
        k=s.ue(); return -(k//2) if k%2==0 else (k+1)//2
    def left(s): return len(s.b)*8-s.p

def rbsp(b):
    out=bytearray(); z=0
    for x in b:
        if z==2 and x==3: z=0; continue
        out.append(x); z=z+1 if x==0 else 0
    return bytes(out)

def nals(data):
    marks=[]; i=0
    while i+3<=len(data):
        if data[i]==0 and data[i+1]==0:
            if data[i+2]==1: marks.append((i,i+3)); i+=3; continue
            if i+4<=len(data) and data[i+2]==0 and data[i+3]==1: marks.append((i,i+4)); i+=4; continue
        i+=1
    out=[]
    for n,(st,ps) in enumerate(marks):
        e=marks[n+1][0] if n+1<len(marks) else len(data)
        out.append(((data[ps]>>1)&0x3f, rbsp(data[ps+2:e])))
    return out

def parse_sps(b):
    r=BR(b)
    vps=r.bits(4); max_sub=r.bits(3)+1; r.bit()
    r.bits(2); r.bit(); r.bits(5); r.bits(32); r.bits(48); r.bits(8)
    if max_sub>1:
        pres=[r.bit() for _ in range(max_sub-1)]
        lev=[r.bit() for _ in range(max_sub-1)]
        r.bits(2*(8-max_sub))
        for i in range(max_sub-1):
            if pres[i]: r.bits(88)
            if lev[i]: r.bits(8)
    sps_id=r.ue(); chroma=r.ue()
    if chroma==3: r.bit()
    w=r.ue(); h=r.ue()
    if r.bit():
        l=r.ue(); rr=r.ue(); t=r.ue(); bo=r.ue()
        sw,sh=(2,2) if chroma==1 else ((2,1) if chroma==2 else (1,1))
        w-=(l+rr)*sw; h-=(t+bo)*sh
    bd_l=r.ue()+8; bd_c=r.ue()+8
    poc_lsb=r.ue()+4
    sub_present=r.bit()
    dpb=0
    for i in range(max_sub if sub_present else 1):
        d=r.ue()+1
        if i==(max_sub if sub_present else 1)-1: dpb=d
        r.ue(); r.ue()
    log2_min_cb=r.ue()+3
    log2_diff_cb=r.ue()
    log2_min_tb=r.ue()+2
    log2_diff_tb=r.ue()
    mtd_inter=r.ue(); mtd_intra=r.ue()
    scaling=r.bit()
    if scaling and r.bit(): raise SystemExit("scaling list data")
    amp=r.bit(); sao=r.bit(); pcm=r.bit()
    pcm_bd_l=pcm_bd_c=pcm_min=pcm_diff=0; pcm_lf=0
    if pcm:
        pcm_bd_l=r.bits(4)+1; pcm_bd_c=r.bits(4)+1
        pcm_min=r.ue()+3; pcm_diff=r.ue(); pcm_lf=r.bit()
    st_rps=r.ue(); lt_present=r.bit(); lt_num=0
    if lt_present:
        lt_num=r.ue()
        for _ in range(lt_num): r.bits(poc_lsb); r.bit()
    tmvp=r.bit(); sis=r.bit()
    return dict(vps=vps,sps_id=sps_id,chroma=chroma,w=w,h=h,bd_l=bd_l,bd_c=bd_c,poc_lsb=poc_lsb,
                log2_min_cb=log2_min_cb,log2_diff_cb=log2_diff_cb,log2_min_tb=log2_min_tb,
                log2_diff_tb=log2_diff_tb,mtd_inter=mtd_inter,mtd_intra=mtd_intra,
                scaling=scaling,amp=amp,sao=sao,pcm=pcm,pcm_bd_l=pcm_bd_l,pcm_bd_c=pcm_bd_c,
                pcm_min=pcm_min,pcm_diff=pcm_diff,pcm_lf=pcm_lf,st_rps=st_rps,
                lt_present=lt_present,lt_num=lt_num,tmvp=tmvp,sis=sis,dpb=dpb)

def parse_pps(b):
    r=BR(b)
    p=dict(pps_id=r.ue(),sps_id=r.ue())
    p['dependent']=r.bit(); p['output']=r.bit(); p['extra']=r.bits(3)
    p['signhide']=r.bit(); p['cabac']=r.bit()
    p['l0']=r.ue()+1; p['l1']=r.ue()+1
    p['qp']=26+r.se(); p['cip']=r.bit(); p['tskip']=r.bit()
    cuqp=r.bit(); p['cuqp']=cuqp; p['diff_cu']=0
    if cuqp: p['diff_cu']=r.ue()
    p['cb']=r.se(); p['cr']=r.se()
    p['slice_chroma']=r.bit(); p['wpred']=r.bit(); p['wbipred']=r.bit(); p['transquant']=r.bit()
    p['tiles']=r.bit(); p['wpp']=r.bit()
    if p['tiles']: raise SystemExit("tiles")
    if p['wpp']: r.bit()
    p['lf_slices']=r.bit() if r.left()>0 else 0
    db=r.bit() if r.left()>0 else 0
    p['db']=db
    p['db_override']=0; p['db_disabled']=0; p['beta']=0; p['tc']=0
    if db and r.left()>=2:
        p['db_override']=r.bit(); p['db_disabled']=r.bit()
        if not p['db_disabled'] and r.left()>=2:
            p['beta']=r.se(); p['tc']=r.se()
    p['scaling']=r.bit() if r.left()>0 else 0
    p['lists_mod']=r.bit() if r.left()>0 else 0
    p['merge']=(r.ue()+2) if r.left()>=4 else 2
    p['ext']=r.bit() if r.left()>0 else 0
    return p

def build(sps,pps,path):
    w=BW()
    # SPS
    w.put(sps['vps'],4); w.put(sps['sps_id'],4); w.put(sps['chroma'],2)
    w.put(sps['w'],16); w.put(sps['h'],16)
    w.put(sps['bd_l'],4); w.put(sps['bd_c'],4); w.put(sps['poc_lsb'],5)
    w.put(sps['log2_diff_cb'],2); w.put(sps['log2_min_cb'],3)
    w.put(sps['log2_min_tb'],3); w.put(sps['log2_diff_tb'],2)
    w.put(sps['mtd_inter'],3); w.put(sps['mtd_intra'],3)
    w.put(sps['scaling'],1); w.put(sps['amp'],1); w.put(sps['sao'],1)
    w.put(sps['pcm'],1)
    w.put(sps['pcm_bd_l'] if sps['pcm'] else 0,4)
    w.put(sps['pcm_bd_c'] if sps['pcm'] else 0,4)
    w.put(sps['pcm_lf'],1); w.put(sps['pcm_diff'],3)
    w.put(sps['pcm_min'] if sps['pcm'] else 0,3)
    w.put(sps['st_rps'],7); w.put(sps['lt_present'],1); w.put(sps['lt_num'],6)
    w.put(sps['tmvp'],1); w.put(sps['sis'],1)
    w.put(0,7); w.put((sps['dpb']-1)&0xf,4); w.put(0,3)
    w.align(32,True)
    # PPS
    w.put(pps['pps_id'],6); w.put(pps['sps_id'],4)
    w.put(pps['dependent'],1); w.put(pps['output'],1); w.put(pps['extra'],13)
    w.put(pps['signhide'],1); w.put(pps['cabac'],1)
    w.put(pps['l0'],4); w.put(pps['l1'],4)
    w.put((pps['qp']-26)&0x7f,7); w.put(pps['cip'],1); w.put(pps['tskip'],1); w.put(pps['cuqp'],1)
    w.put(sps['log2_min_cb']+sps['log2_diff_cb']-pps['diff_cu'],3)
    w.put(pps['cb']&0x1f,5); w.put(pps['cr']&0x1f,5)
    w.put(pps['slice_chroma'],1); w.put(pps['wpred'],1); w.put(pps['wbipred'],1)
    w.put(pps['transquant'],1); w.put(pps['tiles'],1); w.put(pps['wpp'],1)
    w.put(pps['lf_slices'],1); w.put(1,1)
    w.put(pps['db_override'],1); w.put(pps['db_disabled'],1)
    w.put(pps['beta']&0xf,4); w.put(pps['tc']&0xf,4)
    w.put(pps['lists_mod'],1); w.put(pps['merge'],3); w.put(pps['ext'],1)
    w.put(0,3)
    cols=0; rows=0
    w.put(cols,5); w.put(rows,5); w.put(0,4)
    w.align(64,True)
    ctu=1<<(sps['log2_min_cb']+sps['log2_diff_cb'])
    pic_w=(sps['w']+ctu-1)//ctu; pic_h=(sps['h']+ctu-1)//ctu
    for _ in range(20): w.put(max(pic_w-1,0) if _==0 else 0,12)
    for _ in range(22): w.put(max(pic_h-1,0) if _==0 else 0,12)
    w.put(0,32); w.put(0,32); w.put(0,32); w.put(0,6)
    w.align(64,True)
    data=w.bytes(112)
    print("python:", " ".join(f"{b:02x}" for b in data))

data=open(sys.argv[1],'rb').read()
ns=nals(data)
sps=parse_sps([b for t,b in ns if t==33][0])
pps=parse_pps([b for t,b in ns if t==34][0])
print("sps:", {k:sps[k] for k in ('w','h','log2_min_cb','log2_diff_cb','log2_min_tb','log2_diff_tb','st_rps','sao','tmvp','dpb')})
print("pps:", {k:pps[k] for k in ('qp','tiles','wpp','lists_mod','merge','db','beta','tc','diff_cu')})
build(sps,pps,sys.argv[1])
