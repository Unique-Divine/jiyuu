import { useEffect, useMemo } from 'react';
import { Canvas, useThree } from '@react-three/fiber';
import { Html, Line, OrbitControls, PerspectiveCamera } from '@react-three/drei';
import { Color } from 'three';
import { EdgePulseLayer, type EdgePulsePath } from '../graph/EdgePulseLayer';
import { edgeColor } from '../graph/edge-style';
import { useGraphBackgroundReset } from '../graph/useGraphBackgroundReset';
import type { GalaxyArc, GalaxyModel } from './repo-galaxy-model';

export type GalaxyTheme = 'light' | 'dark';

export const GALAXY_PALETTE: Record<GalaxyTheme, { background: string; sphere: string; sphereMissing: string; selected: string; light: string }> = {
    dark: { background: '#101720', sphere: '#91c6df', sphereMissing: '#627384', selected: '#eaf9ff', light: '#e0f5ff' },
    light: { background: '#f3f8fc', sphere: '#2f7193', sphereMissing: '#9eafbd', selected: '#123b52', light: '#ffffff' },
};

/** Edge hues are tuned for a dark ground; darken them on a light one. */
export function arcColor(type: string, theme: GalaxyTheme): string {
    const color = new Color(edgeColor(type));
    if (theme === 'light') color.multiplyScalar(0.72);
    return `#${color.getHexString()}`;
}

function FitCamera({ extent }: { extent: number }) {
    const { camera, controls } = useThree();
    useEffect(() => {
        const distance = extent * 2.1 + 20;
        camera.position.set(0, distance * 0.75, distance);
        camera.lookAt(0, 0, 0);
        camera.far = distance * 10;
        camera.updateProjectionMatrix();
        (controls as unknown as { target?: { set: (x: number, y: number, z: number) => void }; update?: () => void } | null)?.target?.set(0, 0, 0);
        (controls as unknown as { update?: () => void } | null)?.update?.();
    }, [camera, controls, extent]);
    return null;
}

export interface RepoGalaxySceneProps {
    model: GalaxyModel;
    theme: GalaxyTheme;
    active: boolean;
    selectedArc?: string;
    selectedRepo?: string;
    onSelectArc: (arc: GalaxyArc) => void;
    onSelectRepo: (name: string) => void;
    onClear: () => void;
}

export default function RepoGalaxyScene({ model, theme, active, selectedArc, selectedRepo, onSelectArc, onSelectRepo, onClear }: RepoGalaxySceneProps) {
    const palette = GALAXY_PALETTE[theme];
    const background = useGraphBackgroundReset(onClear);
    const focused = useMemo(() => (arc: GalaxyArc) => selectedArc ? arc.id === selectedArc
        : selectedRepo ? arc.from === selectedRepo || arc.to === selectedRepo : true, [selectedArc, selectedRepo]);
    const pulses = useMemo<EdgePulsePath[]>(() => model.arcs.map(arc => ({
        id: arc.id, type: arc.type, color: arcColor(arc.type, theme),
        points: arc.points.map(([x, y, z]) => ({ x, y, z })),
        opacity: focused(arc) ? 0.95 : 0.1,
    })), [model.arcs, theme, focused]);
    return <div className="repo-galaxy-scene" data-testid="repo-galaxy-scene" data-repo-count={model.nodes.length} data-arc-count={model.arcs.length}>
        <Canvas {...background} frameloop={active ? 'always' : 'never'} dpr={[1, 1.5]}
            gl={{ antialias: true, alpha: false, powerPreference: 'low-power' }}
            role="img" aria-label="Repositories as spheres sized by their cross-repo links; arcs run from the repository that uses to the one that is used, one per link kind.">
            <color attach="background" args={[palette.background]} />
            <PerspectiveCamera makeDefault fov={45} near={0.1} far={5000} position={[0, 60, 80]} />
            <OrbitControls makeDefault enabled={active} enableDamping={false} />
            <FitCamera extent={model.extent} />
            <ambientLight intensity={theme === 'light' ? 2.2 : 1.4} />
            <directionalLight position={[40, 80, 30]} intensity={2.2} color={palette.light} />
            {model.arcs.map(arc => {
                const on = focused(arc), chosen = arc.id === selectedArc;
                return <group key={arc.id} onClick={event => { event.stopPropagation(); onSelectArc(arc); }}>
                    <Line points={arc.points} color={arcColor(arc.type, theme)} lineWidth={chosen ? arc.width + 1.5 : arc.width}
                        transparent opacity={on ? (chosen ? 1 : 0.7) : 0.08} depthWrite={false} />
                </group>;
            })}
            <EdgePulseLayer paths={pulses} active={active} />
            {model.nodes.map(node => {
                const chosen = node.name === selectedRepo;
                return <group key={node.name} position={node.position}>
                    <mesh onClick={event => { event.stopPropagation(); onSelectRepo(node.name); }}>
                        <sphereGeometry args={[node.radius, 32, 24]} />
                        <meshStandardMaterial color={chosen ? palette.selected : node.indexed ? palette.sphere : palette.sphereMissing}
                            roughness={0.55} metalness={0.1} />
                    </mesh>
                    <Html position={[0, node.radius + 1.6, 0]} center zIndexRange={[20, 10]} style={{ pointerEvents: 'none' }}>
                        <span className="repo-galaxy-label" data-selected={chosen}>
                            {node.name}<small>{node.links.toLocaleString()} links{node.indexed ? '' : ' · not indexed'}</small>
                        </span>
                    </Html>
                </group>;
            })}
        </Canvas>
    </div>;
}
