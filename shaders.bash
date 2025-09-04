set -ex

for name in egui triangle tonemap environment; do
    dxc ./crates/render/shaders/hlsl/${name}.hlsl -I ./crates/render/shaders/hlsl -T vs_6_2 -E vs_main -enable-16bit-types -Zi -O0 -Qembed_debug -Fo ./crates/render/shaders/dxil/${name}-debug.vs.cso
    dxc ./crates/render/shaders/hlsl/${name}.hlsl -I ./crates/render/shaders/hlsl -T vs_6_2 -E vs_main -enable-16bit-types -Zi -Qembed_debug -Fo ./crates/render/shaders/dxil/${name}.vs.cso
    dxc ./crates/render/shaders/hlsl/${name}.hlsl -I ./crates/render/shaders/hlsl -T ps_6_2 -E ps_main -enable-16bit-types -Zi -O0 -Qembed_debug -Fo ./crates/render/shaders/dxil/${name}-debug.ps.cso
    dxc ./crates/render/shaders/hlsl/${name}.hlsl -I ./crates/render/shaders/hlsl -T ps_6_2 -E ps_main -enable-16bit-types -Zi -Qembed_debug -Fo ./crates/render/shaders/dxil/${name}.ps.cso
done
