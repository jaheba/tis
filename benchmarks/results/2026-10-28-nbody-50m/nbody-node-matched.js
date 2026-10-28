
/* The Computer Language Benchmarks Game
   https://salsa.debian.org/benchmarksgame-team/benchmarksgame/
   contributed by Isaac Gouy
   modified by Andrey Filatkin */

const PI = Math.PI;
const SOLAR_MASS = 4 * PI * PI;
const DAYS_PER_YEAR = 365.24;

function Body(x, y, z, vx, vy, vz, mass) {
    return [[x, y, z], [vx, vy, vz], mass];
}

function Jupiter() {
    return new Body(
        4.84143144246472090e+00,
        -1.16032004402742839e+00,
        -1.03622044471123109e-01,
        1.66007664274403694e-03 * DAYS_PER_YEAR,
        7.69901118419740425e-03 * DAYS_PER_YEAR,
        -6.90460016972063023e-05 * DAYS_PER_YEAR,
        9.54791938424326609e-04 * SOLAR_MASS
    );
}

function Saturn() {
    return new Body(
        8.34336671824457987e+00,
        4.12479856412430479e+00,
        -4.03523417114321381e-01,
        -2.76742510726862411e-03 * DAYS_PER_YEAR,
        4.99852801234917238e-03 * DAYS_PER_YEAR,
        2.30417297573763929e-05 * DAYS_PER_YEAR,
        2.85885980666130812e-04 * SOLAR_MASS
    );
}

function Uranus() {
    return new Body(
        1.28943695621391310e+01,
        -1.51111514016986312e+01,
        -2.23307578892655734e-01,
        2.96460137564761618e-03 * DAYS_PER_YEAR,
        2.37847173959480950e-03 * DAYS_PER_YEAR,
        -2.96589568540237556e-05 * DAYS_PER_YEAR,
        4.36624404335156298e-05 * SOLAR_MASS
    );
}

function Neptune() {
    return new Body(
        1.53796971148509165e+01,
        -2.59193146099879641e+01,
        1.79258772950371181e-01,
        2.68067772490389322e-03 * DAYS_PER_YEAR,
        1.62824170038242295e-03 * DAYS_PER_YEAR,
        -9.51592254519715870e-05 * DAYS_PER_YEAR,
        5.15138902046611451e-05 * SOLAR_MASS
    );
}

function Sun() {
    return new Body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, SOLAR_MASS);
}

const bodies = Array(Sun(), Jupiter(), Saturn(), Uranus(), Neptune());

// The kernel below follows tis/tests/fixtures/game_nbody.py directly:
// plain nested arrays, precomputed pairs, matching arithmetic and update order.
const pairs = [];
for (let i = 0; i < bodies.length - 1; i++) {
    for (const other of bodies.slice(i + 1)) pairs.push([bodies[i], other]);
}

function offsetMomentum(ref) {
    let px = 0.0, py = 0.0, pz = 0.0;
    for (const [r, [vx, vy, vz], m] of bodies) {
        px -= vx * m;
        py -= vy * m;
        pz -= vz * m;
    }
    const [r, v, m] = ref;
    v[0] = px / m;
    v[1] = py / m;
    v[2] = pz / m;
}

function advance(dt, n) {
    for (let i = 0; i < n; i++) {
        for (const [[[x1, y1, z1], v1, m1], [[x2, y2, z2], v2, m2]] of pairs) {
            const dx = x1 - x2;
            const dy = y1 - y2;
            const dz = z1 - z2;
            const mag = dt * ((dx * dx + dy * dy + dz * dz) ** (-1.5));
            const b1m = m1 * mag;
            const b2m = m2 * mag;
            v1[0] -= dx * b2m;
            v1[1] -= dy * b2m;
            v1[2] -= dz * b2m;
            v2[0] += dx * b1m;
            v2[1] += dy * b1m;
            v2[2] += dz * b1m;
        }
        for (const [r, [vx, vy, vz], m] of bodies) {
            r[0] += dt * vx;
            r[1] += dt * vy;
            r[2] += dt * vz;
        }
    }
}

function energy() {
    let e = 0.0;
    for (const [[[x1, y1, z1], v1, m1], [[x2, y2, z2], v2, m2]] of pairs) {
        const dx = x1 - x2;
        const dy = y1 - y2;
        const dz = z1 - z2;
        e -= (m1 * m2) / ((dx * dx + dy * dy + dz * dz) ** 0.5);
    }
    for (const [r, [vx, vy, vz], m] of bodies) {
        e += m * (vx * vx + vy * vy + vz * vz) / 2.0;
    }
    return e;
}

const n = +process.argv[2];
offsetMomentum(bodies[0]);
const saved = bodies.map(([r, v]) => [r.slice(), v.slice()]);
for (let warmup = 0; warmup < 2; warmup++) {
    advance(0.01, 20000);
    for (let i = 0; i < bodies.length; i++) {
        for (let j = 0; j < 3; j++) {
            bodies[i][0][j] = saved[i][0][j];
            bodies[i][1][j] = saved[i][1][j];
        }
    }
}
console.log(energy().toFixed(9));
advance(0.01, n);
console.log(energy().toFixed(9));
