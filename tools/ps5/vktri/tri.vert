#version 450
// Vertex-less triangle (gl_VertexIndex) for the PS5 Vulkan bring-up test.
layout(location = 0) out vec3 colour;
layout(push_constant) uniform Push { float phase; } push;
void main() {
    const vec2 p[3] = vec2[](vec2(0.0, -0.7), vec2(0.7, 0.6), vec2(-0.7, 0.6));
    const vec3 c[3] = vec3[](vec3(1, 0, 0), vec3(0, 1, 0), vec3(0, 0, 1));
    float s = sin(push.phase), k = cos(push.phase);
    vec2 v = p[gl_VertexIndex];
    gl_Position = vec4(v.x * k - v.y * s, v.x * s + v.y * k, 0.0, 1.0);
    colour = c[gl_VertexIndex];
}
