/**
 * PCB Copper Shader - HASL-finish copper for PCB pads and traces.
 * Flat metallic appearance with warm gold/copper tones.
 */

import * as THREE from "three";
import type { ProceduralShaderDef } from "../types";

const vertexShader = /* glsl */ `
#include <common>
#include <logdepthbuf_pars_vertex>

varying vec3 vWorldPosition;
varying vec3 vWorldNormal;

void main() {
  vec4 worldPos = modelMatrix * vec4(position, 1.0);
  vWorldPosition = worldPos.xyz;
  vWorldNormal = normalize((modelMatrix * vec4(normal, 0.0)).xyz);
  gl_Position = projectionMatrix * viewMatrix * worldPos;

  #include <logdepthbuf_vertex>
}
`;

const fragmentShader = /* glsl */ `
#include <logdepthbuf_pars_fragment>

uniform vec3 uBaseColor;
uniform float uMetalness;
uniform float uRoughness;

varying vec3 vWorldPosition;
varying vec3 vWorldNormal;

void main() {
  vec3 normal = normalize(vWorldNormal);

  // Simple PBR-like lighting
  vec3 lightDir = normalize(vec3(0.5, 1.0, 0.5));
  float diffuse = max(dot(normal, lightDir), 0.0);
  float ambient = 0.3;

  // Specular highlight
  vec3 viewDir = normalize(cameraPosition - vWorldPosition);
  vec3 halfDir = normalize(lightDir + viewDir);
  float spec = pow(max(dot(normal, halfDir), 0.0), mix(16.0, 128.0, 1.0 - uRoughness));

  // Metallic reflection tinted by base color
  vec3 reflectColor = uBaseColor * spec * uMetalness;

  vec3 finalColor = uBaseColor * (ambient + diffuse * 0.7) + reflectColor;

  gl_FragColor = vec4(finalColor, 1.0);

  #include <logdepthbuf_fragment>
}
`;

export const pcbCopperShader: ProceduralShaderDef = {
  key: "pcb-copper",
  vertexShader,
  fragmentShader,
  uniforms: {
    uBaseColor: { value: new THREE.Color(0.84, 0.68, 0.37) },
    uMetalness: { value: 0.95 },
    uRoughness: { value: 0.15 },
  },
};
