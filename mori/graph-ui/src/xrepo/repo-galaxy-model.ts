/**
 * Pure layout for the Repos view: repositories as spheres on a ring, one arc
 * per (from, to, type) aggregate. No three.js here, so it is testable and
 * the scene only draws what this returns.
 */
import type { XrepoAggregate, XrepoRepo, XrepoSummary } from './xrepo-api';

export type Point3 = [number, number, number];

export interface GalaxyRepoNode {
    name: string;
    position: Point3;
    radius: number;
    /** Links in and out, over the enabled types. */
    links: number;
    indexed: boolean;
    nodes: number;
}

export interface GalaxyArc {
    /** `${from}->${to}:${type}`; stable across reloads. */
    id: string;
    from: string;
    to: string;
    type: string;
    count: number;
    /** Line width in CSS pixels, by log count. */
    width: number;
    points: Point3[];
}

export interface GalaxyModel {
    nodes: GalaxyRepoNode[];
    arcs: GalaxyArc[];
    /** Radius that holds every node, for the camera. */
    extent: number;
}

export interface GalaxyOptions {
    /** Types to draw; undefined draws every type. */
    types?: ReadonlySet<string>;
    /** Also place repositories without any (enabled) link. */
    showUnlinked?: boolean;
}

const KNOWN_TYPE_LABELS: Record<string, string> = {
    GRAPHQL: 'GraphQL', HTTP: 'HTTP', GRPC: 'gRPC', TRPC: 'tRPC', ASYNC: 'Async', CHANNEL: 'Channel',
    IMPORTS: 'Import', IMPORT: 'Import', DOC_REF: 'Doc reference', DOCS: 'Doc reference',
};

/** "CROSS_GRAPHQL_CALLS" -> "GraphQL"; unknown kinds read as title case. */
export function typeLabel(type: string): string {
    const core = type.replace(/^CROSS_/, '').replace(/_CALLS$/, '');
    if (KNOWN_TYPE_LABELS[core]) return KNOWN_TYPE_LABELS[core];
    return core.split('_').filter(Boolean)
        .map(word => word[0] + word.slice(1).toLowerCase()).join(' ') || type;
}

export const arcId = (link: Pick<XrepoAggregate, 'from' | 'to' | 'type'>): string =>
    `${link.from}->${link.to}:${link.type}`;

export function arcWidth(count: number): number {
    return Math.min(7, 1.2 + 1.8 * Math.log10(Math.max(1, count)));
}

export function sphereRadius(links: number): number {
    return Math.min(6, 1.2 + 0.6 * Math.log2(1 + Math.max(0, links)));
}

const add = (a: Point3, b: Point3): Point3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
const scale = (a: Point3, s: number): Point3 => [a[0] * s, a[1] * s, a[2] * s];
const sub = (a: Point3, b: Point3): Point3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const length = (a: Point3): number => Math.hypot(a[0], a[1], a[2]);

/** A raised quadratic curve from a to b. `lane` separates arcs of one pair:
 * each type gets its own height, and the side depends on direction, so A->B
 * and B->A never overlap. */
export function arcPoints(a: Point3, b: Point3, lane: number, segments = 28): Point3[] {
    const span = sub(b, a);
    const distance = length(span) || 1;
    const mid = scale(add(a, b), 0.5);
    // Horizontal normal to the span (the ring lies in XZ).
    const side: Point3 = [-span[2] / distance, 0, span[0] / distance];
    const lift = distance * 0.22 + 3 + lane * 2.4;
    const control = add(add(mid, [0, lift, 0]), scale(side, distance * 0.12 + lane * 1.2));
    const points: Point3[] = [];
    for (let i = 0; i <= segments; i++) {
        const t = i / segments, u = 1 - t;
        points.push([
            u * u * a[0] + 2 * u * t * control[0] + t * t * b[0],
            u * u * a[1] + 2 * u * t * control[1] + t * t * b[1],
            u * u * a[2] + 2 * u * t * control[2] + t * t * b[2],
        ]);
    }
    return points;
}

/** Order repos so the busiest sit apart: the two with the most links take
 * opposite slots, so their arc spans the ring instead of joining
 * neighbours; the rest fill the gaps by rank, alternating sides. */
export function ringOrder<T extends { name: string; links: number }>(repos: T[]): T[] {
    const sorted = [...repos].sort((x, y) => y.links - x.links || x.name.localeCompare(y.name));
    const n = sorted.length, half = Math.floor(n / 2);
    const slots = [0, half];
    for (let k = 1; slots.length < n; k++) {
        for (const slot of [half + k, k, n - k, half - k]) {
            if (slot > 0 && slot < n && !slots.includes(slot)) slots.push(slot);
        }
    }
    const ring = new Array<T>(n);
    sorted.forEach((repo, index) => { ring[slots[index]] = repo; });
    return n === 1 ? sorted : ring;
}

export function buildGalaxyModel(summary: XrepoSummary, options: GalaxyOptions = {}): GalaxyModel {
    const enabled = (link: XrepoAggregate) => !options.types || options.types.has(link.type);
    const links = summary.links.filter(enabled);
    const totals = new Map<string, number>();
    for (const link of links) {
        totals.set(link.from, (totals.get(link.from) ?? 0) + link.count);
        totals.set(link.to, (totals.get(link.to) ?? 0) + link.count);
    }
    const byName = new Map<string, XrepoRepo>(summary.repos.map(repo => [repo.name, repo]));
    for (const name of totals.keys()) {
        if (!byName.has(name)) byName.set(name, { name, indexed: false, nodes: 0, edges: 0, linksIn: 0, linksOut: 0 });
    }
    const candidates = [...byName.values()]
        .filter(repo => options.showUnlinked || totals.has(repo.name))
        .map(repo => {
            const count = totals.get(repo.name) ?? 0;
            return { name: repo.name, position: [0, 0, 0] as Point3, radius: sphereRadius(count), links: count,
                indexed: repo.indexed, nodes: repo.nodes };
        });
    const nodes = ringOrder(candidates);
    const ringRadius = Math.max(32, nodes.reduce((sum, node) => sum + node.radius * 2 + 18, 0) / (2 * Math.PI));
    nodes.forEach((node, index) => {
        const angle = (index / Math.max(1, nodes.length)) * Math.PI * 2;
        const wave = nodes.length > 2 ? Math.sin(angle * 3) * ringRadius * 0.12 : 0;
        node.position = [Math.cos(angle) * ringRadius, wave, Math.sin(angle) * ringRadius];
    });
    const placed = new Map(nodes.map(node => [node.name, node]));
    const typeOrder = [...new Set(summary.types.length ? summary.types : links.map(link => link.type))];
    const arcs: GalaxyArc[] = [];
    for (const link of links) {
        const a = placed.get(link.from), b = placed.get(link.to);
        if (!a || !b || a === b) continue;
        const lane = Math.max(0, typeOrder.indexOf(link.type));
        arcs.push({ id: arcId(link), from: link.from, to: link.to, type: link.type, count: link.count,
            width: arcWidth(link.count), points: arcPoints(a.position, b.position, lane) });
    }
    const extent = nodes.reduce((max, node) => Math.max(max, length(node.position) + node.radius), 10);
    return { nodes, arcs, extent };
}
