import sys,struct
for fn in sys.argv[1:]:
    d=open(fn,'rb').read(); pe=struct.unpack('<I',d[0x3c:0x40])[0]
    ns=struct.unpack('<H',d[pe+6:pe+8])[0]; ts=struct.unpack('<I',d[pe+8:pe+12])[0]; osz=struct.unpack('<H',d[pe+20:pe+22])[0]
    opt=pe+24; ib=struct.unpack('<I',d[opt+28:opt+32])[0]
    dd=opt+96; exp_rva,exp_sz=struct.unpack('<II',d[dd:dd+8]); imp_rva,_=struct.unpack('<II',d[dd+8:dd+16]); rel_rva,rel_sz=struct.unpack('<II',d[dd+40:dd+48])
    secs=[]; so=opt+osz
    for i in range(ns):
        s=d[so+i*40:so+i*40+40]; secs.append((s[:8].rstrip(b'\0').decode(),*struct.unpack('<IIII',s[8:24])))
    def r2o(r):
        for n,vs,va,rs,ro in secs:
            if va<=r<va+max(vs,rs): return r-va+ro
    names=[]
    if exp_rva:
        e=r2o(exp_rva); nn=struct.unpack('<I',d[e+24:e+28])[0]; an=r2o(struct.unpack('<I',d[e+32:e+36])[0])
        for i in range(nn):
            o=r2o(struct.unpack('<I',d[an+i*4:an+i*4+4])[0]); names.append(d[o:d.index(b'\0',o)].decode())
    print(fn.split('/')[-1],'imgbase',hex(ib),'secs',[(s[0],s[1]) for s in secs],'reloc',rel_sz,'imports',bool(imp_rva),'exports',len(names),names[:12])
