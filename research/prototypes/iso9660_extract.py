import sys,struct,os
f=open(sys.argv[1],'rb'); out=sys.argv[2] if len(sys.argv)>2 else None
def sec(n,c=1):
    b=b''
    for i in range(c):
        f.seek((n+i)*2352+16); b+=f.read(2048)
    return b
pvd=sec(16); assert pvd[1:6]==b'CD001'
print("volid",pvd[40:72].decode().strip(),"created",pvd[813:830].decode())
def walk(lba,size,path):
    d=sec(lba,(size+2047)//2048); i=0
    while i<len(d):
        l=d[i]
        if l==0: i=(i//2048+1)*2048; continue
        ext=struct.unpack('<I',d[i+2:i+6])[0]; sz=struct.unpack('<I',d[i+10:i+14])[0]
        fl=d[i+25]; nl=d[i+32]; name=d[i+33:i+33+nl]
        i+=l
        if name in (b'\0',b'\1'): continue
        n=name.decode('latin1').split(';')[0]; p=path+'/'+n
        if fl&2: walk(ext,sz,p)
        else:
            print(f"{sz:>11} {p}")
            if out:
                os.makedirs(os.path.dirname(out+p),exist_ok=True)
                with open(out+p,'wb') as o:
                    rem=sz;k=ext
                    while rem>0:
                        o.write(sec(k)[:min(2048,rem)]); rem-=2048;k+=1
r=pvd[156:190]; walk(struct.unpack('<I',r[2:6])[0],struct.unpack('<I',r[10:14])[0],'')
