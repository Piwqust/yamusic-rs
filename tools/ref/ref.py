# Эталон: оригинальный фрагментный шейдер воркера на настоящем OpenGL, вне экрана.
import sys, math, numpy as np, moderngl
from PIL import Image
frag_path, out, time, audio, react = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5])
frag = open(frag_path, encoding='utf-8').read()
frag = '#version 330\nout vec4 fragColor;\n' + frag.replace('gl_FragColor', 'fragColor')
vert = '#version 330\nin vec4 position;\nvoid main(){gl_Position=position;}'
ctx = moderngl.create_standalone_context()
prog = ctx.program(vertex_shader=vert, fragment_shader=frag)
vbo = ctx.buffer(np.array([-1,-1,0,1, 1,-1,0,1, -1,1,0,1, 1,1,0,1], dtype='f4').tobytes())
vao = ctx.vertex_array(prog, [(vbo, '4f', 'position')])
def hsl(t, e, i):
    def s(s_):
        n = (s_ + t/30) % 12; r = e*min(i, 1-i)
        return i - r*max(-1, min(n-3, 9-n, 1))
    return [s(0), s(8), s(4)]
top, middle, bottom = hsl(271, .717, .58), hsl(271, .717, .53), hsl(271, .567, .48)
def setu(name, value):
    if name in prog: prog[name].value = value
setu('vScreenSize', (650.0, 650.0)); setu('vTime', time); setu('vScale', 0.35)
prog['vColor'].write(np.array(bottom+middle+top+bottom+middle+top, dtype='f4').tobytes())
prog['vRotation'].write(np.array([-.3,.3,.2,-.3,-.3,-.2,-.3,-.3,.2], dtype='f4').tobytes())
if 'vAudio' in prog: prog['vAudio'].write(np.array([audio]*3, dtype='f4').tobytes())
if 'vReact' in prog: prog['vReact'].write(np.array([react]*3, dtype='f4').tobytes())
fbo = ctx.framebuffer(color_attachments=[ctx.texture((650, 650), 4, dtype='f4')])
fbo.use(); ctx.viewport = (0, 0, 650, 650); fbo.clear(0, 0, 0, 0)
vao.render(moderngl.TRIANGLE_STRIP)
px = np.frombuffer(fbo.read(components=4, dtype='f4'), dtype='f4').reshape(650, 650, 4)[::-1]
rgb = np.clip(px[..., :3], 0, 1); a = np.clip(px[..., 3:4], 0, 1)
# Браузер: холст без premultipliedAlpha поверх фона страницы #121212.
img = rgb*a + (18/255)*(1-a)
Image.fromarray((img*255+0.5).astype('u1')).save(out)
print('ok', ctx.info['GL_RENDERER'])
