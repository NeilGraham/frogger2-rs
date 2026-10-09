import sys,struct,glob,os
D=sys.argv[1]
def fla(d):
    assert d[:4]==b'FLA2'; n=struct.unpack('<I',d[4:8])[0]
    out=bytearray(); hist=bytearray(4096); hp=0; i=8
    while True:
        tag=d[i]; i+=1
        for _ in range(8):
            if tag&0x80:
                c=d[i]; i+=1
                if c==0: return bytes(out),n
                off=4096-(((c&0xF0)*16)+d[i]); i+=1; ln=(c&0x0F)+2
                for _ in range(ln):
                    b=hist[(hp+off)&4095]; out.append(b); hist[hp]=b; hp=(hp+1)&4095
            else:
                b=d[i]; i+=1; out.append(b); hist[hp]=b; hp=(hp+1)&4095
            tag=(tag<<1)&0xff
ok=0
for f in sorted(glob.glob(D+'/textures/*.fla'))+[D+'/maps/language.fla']:
    raw=open(f,'rb').read(); d,n=fla(raw)
    st='size-ok' if len(d)==n else f'SIZE MISMATCH {len(d)} vs {n}'
    info=''
    if '/textures/' in f:
        cnt=struct.unpack('<I',d[:4])[0]; o=4; names=[]
        try:
            for k in range(cnt):
                fl,crc,w,h=struct.unpack('<IIhh',d[o:o+12]); nm=d[o+12:o+24].split(b'\0')[0].decode('latin1'); o+=24+w*h*2; names.append((nm,w,h))
            na=struct.unpack('<I',d[o:o+4])[0]; o+=4
            for k in range(na):
                nf=struct.unpack('<I',d[o:o+4])[0]; o+=8+nf*8
            info=f'tex={cnt} anims={na} consumed={o}/{len(d)} e.g.{names[:2]}'
        except Exception as e: info=f'tex parse fail {e} cnt={cnt}'
    else: info=repr(d[:120])
    print(os.path.basename(f),st,info)
for f in sorted(glob.glob(D+'/sfx/*.wus')):
    d=open(f,'rb').read(); n=struct.unpack('<I',d[:4])[0]; o=4; fmts=set()
    for k in range(n):
        ln,uid,lp=struct.unpack('<III',d[o:o+12]); tag,ch,sr=struct.unpack('<HHI',d[o+12:o+20]); bits=struct.unpack('<H',d[o+26:o+28])[0]; fmts.add((tag,ch,sr,bits)); o+=12+18+ln
    print(os.path.basename(f),'samples',n,'consumed',o,'/',len(d),fmts)
