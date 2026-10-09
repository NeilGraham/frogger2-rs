import sys,struct
d=open(sys.argv[1],'rb').read(); pe=struct.unpack('<I',d[0x3c:0x40])[0]
ns=struct.unpack('<H',d[pe+6:pe+8])[0]; osz=struct.unpack('<H',d[pe+20:pe+22])[0]; opt=pe+24; ib=struct.unpack('<I',d[opt+28:opt+32])[0]
secs=[]
for i in range(ns):
    s=d[opt+osz+i*40:opt+osz+i*40+40]; secs.append(struct.unpack('<IIII',s[8:24]))
def va(v):
    r=v-ib
    for vs,a,rs,ro in secs:
        if a<=r<a+max(vs,rs): return r-a+ro
    raise ValueError(hex(v))
u32=lambda o:struct.unpack('<I',d[o:o+4])[0]; i16=lambda o:struct.unpack('<h',d[o:o+2])[0]
dd=opt+96; e=va(ib+u32(dd)); fn=va(ib+u32(va(ib+u32(e+28))))
code=d[fn:fn+16]; print('GetbankAddress code',code.hex())
p=code.index(b"\xb8"); arr=va(struct.unpack("<I",code[p+1:p+5])[0])
k=0
while arr is not None:
    oid,ctl=u32(arr+8*k),u32(arr+8*k+4)
    if ctl==0: break
    c=va(ctl); obj=u32(c+12)
    # MDX_OBJECT: objID u32, name[16], pad i16, xlu u8, numSprites u8, sprites*, mesh*
    o=va(obj); name=d[o+4:o+20].split(b'\0')[0].decode('latin1'); mesh=u32(o+28)
    nf=nv=0
    if mesh: m=va(mesh); nf,nv=i16(m),i16(m+2)
    if k<12: print(f'  obj {k}: id={oid:08x} name={name!r} mesh faces={nf} verts={nv}')
    k+=1
print('total objects',k)
