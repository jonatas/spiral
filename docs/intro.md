---
layout: post
title: "Spiral: An Experiment in Geometry-Aware Storage for PostgreSQL"
categories: ['postgresql', 'rust', 'engineering']
tags: ['spiral', 'research', 'database-internals', 'z-order', 'time-series']
description: "A look into Spiral: my research project on multidimensional storage engines, built as a way to learn PostgreSQL internals through Rust."
permalink: /spiral-intro
mermaid: true
math: true
image: images/postgresql-performance-workshop.webp
---

<style>
code {
  background-color: rgba(14, 165, 233, 0.15) !important;
  color: #7dd3fc !important;
  padding: 0.11rem 0.35rem !important;
  border-radius: 4px !important;
  font-weight: 500 !important;
  border: 1px solid rgba(14, 165, 233, 0.2) !important;
}
.math-formula {
  background: rgba(15, 23, 42, 0.5);
  padding: 1.5rem;
  border-radius: 8px;
  border: 1px solid #1e293b;
  margin: 1.5rem 0;
  overflow-x: auto;
  text-align: center;
}
.interactive-widget {
  box-shadow: 0 20px 50px rgba(0,0,0,0.4);
}
</style>

# A Personal Journey into Database Internals

I should start with a confession: I am not a math expert, nor am I a core PostgreSQL developer. I am a curious engineer who loves to experiment. **Spiral** is the result of letting my creative side take the wheel—a research project born from a simple desire to understand how databases work at the bit-level and to see if I could implement some of my own "crazy" ideas.

Everything you see here is a **prototype**. It is an exploration of what becomes possible when we use Rust to extend the heart of the database. I'm sharing this as a research outcome, hoping to find other curious minds who might want to contribute or challenge these concepts.

# The "Spiral" Concept: A Metaphor for Data Flow

The central idea behind this project is to stop thinking of data as a flat, linear history. As datasets grow to billions of rows, the traditional model faces what I call **Data Gravity**—where the weight of the data makes every operation exponentially harder.

In my experiments, I started imagining storage as a **Spiral**. 

<div id="cosmic-spiral-root" class="interactive-widget" style="margin: 2rem 0; background: #020617; padding: 3rem 2rem; border-radius: 12px; border: 1px solid #1e293b; display: flex; flex-direction: column; align-items: center; overflow: hidden; height: 450px; position: relative;">
  <svg id="spiral-svg" width="350" height="350" viewBox="0 0 400 400">
    <circle cx="200" cy="200" r="180" fill="none" stroke="#1e293b" stroke-width="1" stroke-dasharray="5,5" />
    <circle cx="200" cy="200" r="130" fill="none" stroke="#1e293b" stroke-width="1" stroke-dasharray="5,5" />
    <circle cx="200" cy="200" r="80" fill="none" stroke="#1e293b" stroke-width="1" stroke-dasharray="5,5" />
    <circle cx="200" cy="200" r="10" fill="#0ea5e9" />
  </svg>
  <div style="position: absolute; bottom: 30px; color: #64748b; font-family: 'JetBrains Mono', monospace; font-size: 0.8rem; text-align: center; line-height: 1.5;">
    <strong style="color: #fff;">THE SPIRAL METAPHOR</strong><br>
    Inner Core: High-velocity raw ticks<br>
    Outer Lanes: Hierarchical rollups
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const svg = document.getElementById('spiral-svg'); if (!svg) return;
    const center = 200; const lanes = [80, 130, 180]; const colors = ['#38bdf8', '#818cf8', '#c084fc'];
    function createStar() {
      const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
      const laneIdx = Math.floor(Math.random() * 3); const radius = lanes[laneIdx];
      let angle = Math.random() * Math.PI * 2; circle.setAttribute('r', '3'); circle.setAttribute('fill', colors[laneIdx]); svg.appendChild(circle);
      function animate() { angle += (0.01 / (laneIdx + 1)); const x = center + radius * Math.cos(angle); const y = center + radius * Math.sin(angle); circle.setAttribute('cx', x); circle.setAttribute('cy', y); requestAnimationFrame(animate); }
      animate();
    }
    for(let i=0; i<20; i++) createStar();
  });
})();
</script>

In this model, fresh data arrives at high velocity in the center. As it "cools" and ages, it migrates to outer orbits where it's aggregated into stable, dense structures.

# Multi-Tenancy: The Lane System

In a multi-tenant environment, the "Data Gravity" problem is multiplied. If we store everyone's data in the same linear file, noisy neighbors can drown out everyone else. 

Spiral solves this by assigning each tenant their own **Lane** within the spiral. Imagine a cosmic highway where each tenant has their own orbital path. This preserves locality not just in time, but in tenant identity. You can explore this concept in depth at the [Spiral Interactive Lab](../spiral).

<div id="multi-tenant-spiral-root" class="interactive-widget" style="margin: 2rem 0; background: #020617; padding: 2rem; border-radius: 12px; border: 1px solid #1e293b; display: flex; flex-direction: column; align-items: center; overflow: hidden; height: 550px; position: relative;">
  <canvas id="multi-tenant-canvas" width="600" height="450" style="width: 100%; height: 100%; max-width: 600px;"></canvas>
  
  <div style="position: absolute; top: 20px; right: 20px; display: flex; flex-direction: column; gap: 10px;">
    <button id="add-tenant-btn" style="background: #0ea5e9; color: #fff; border: none; padding: 8px 16px; border-radius: 4px; font-size: 0.8rem; cursor: pointer; font-family: 'JetBrains Mono', monospace; font-weight: bold; box-shadow: 0 0 15px rgba(14, 165, 233, 0.4);">+ ADD TENANT</button>
  </div>

  <div id="spiral-slides" style="position: absolute; bottom: 20px; left: 20px; right: 20px; background: rgba(15, 23, 42, 0.85); backdrop-filter: blur(8px); padding: 1.2rem; border-radius: 10px; border: 1px solid #1e3a8a; color: #94a3b8; font-family: 'JetBrains Mono', monospace; font-size: 0.75rem; line-height: 1.5; box-shadow: 0 10px 30px rgba(0,0,0,0.5);">
    <div class="spiral-slide" data-step="0">
      <strong style="color: #4ade80;">[01] THE SINGLE TENANT FLOW</strong><br>
      Data originates at the high-velocity center (Hot Storage). As it ages, the algorithm pushes it outward, creating a natural temporal gradient.
    </div>
    <div class="spiral-slide" data-step="1" style="display: none;">
      <strong style="color: #60a5fa;">[02] OPENING THE LANES</strong><br>
      The space is mathematically partitioned into lanes. Each lane represents a different aggregation level (Raw -> 1m -> 1h -> 1d).
    </div>
    <div class="spiral-slide" data-step="2" style="display: none;">
      <strong style="color: #c084fc;">[03] SCALABLE MULTI-TENANCY</strong><br>
      By shifting the orbital phase, we stack multiple tenants into the same table. They share the same physical "lane" boundaries but never collide in bit-space.
    </div>
  </div>
  
  <div style="position: absolute; bottom: 25px; right: 30px; display: flex; gap: 8px;">
    <button id="prev-slide" style="background: #1e293b; color: #fff; border: 1px solid #334155; padding: 6px 12px; border-radius: 4px; cursor: pointer; transition: all 0.2s;">&lt;</button>
    <button id="next-slide" style="background: #1e293b; color: #fff; border: 1px solid #334155; padding: 6px 12px; border-radius: 4px; cursor: pointer; transition: all 0.2s;">&gt;</button>
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const canvas = document.getElementById('multi-tenant-canvas');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    const addBtn = document.getElementById('add-tenant-btn');
    const prevBtn = document.getElementById('prev-slide');
    const nextBtn = document.getElementById('next-slide');
    const slides = document.querySelectorAll('.spiral-slide');
    
    let currentStep = 0;
    let time = 0;
    let tenants = [
      { id: 0, color: '#0ea5e9', phase: 0, points: [] }
    ];

    function updateSlides() {
      slides.forEach((s, i) => {
        s.style.display = i === currentStep ? 'block' : 'none';
        s.style.animation = 'fadeIn 0.5s ease-out';
      });
    }

    nextBtn.onclick = () => { currentStep = (currentStep + 1) % slides.length; updateSlides(); };
    prevBtn.onclick = () => { currentStep = (currentStep - 1 + slides.length) % slides.length; updateSlides(); };

    addBtn.onclick = () => {
      const colors = ['#f43f5e', '#10b981', '#f59e0b', '#8b5cf6', '#ec4899', '#f97316'];
      const newPhase = (tenants.length * Math.PI * 2) / Math.max(5, tenants.length + 1);
      tenants.push({
        id: tenants.length,
        color: colors[tenants.length % colors.length],
        phase: newPhase,
        points: []
      });
      if (currentStep < 2) { currentStep = 2; updateSlides(); }
    };

    function draw() {
      const w = canvas.width;
      const h = canvas.height;
      ctx.clearRect(0, 0, w, h);
      
      const centerX = w / 2;
      const centerY = h / 2;
      
      time += 0.01;
      const globalRotation = time * 0.15;
      const zoom = 1.1 + Math.sin(time * 0.4) * 0.1;

      // Draw background grid/stars
      ctx.fillStyle = 'rgba(30, 41, 59, 0.2)';
      for(let i=0; i<50; i++) {
        const x = (Math.sin(i * 123.45) * 0.5 + 0.5) * w;
        const y = (Math.cos(i * 543.21) * 0.5 + 0.5) * h;
        ctx.fillRect(x, y, 1, 1);
      }

      // Draw background lanes
      if (currentStep >= 1) {
        ctx.strokeStyle = '#1e293b';
        ctx.lineWidth = 1;
        for (let r = 50; r < 250; r += 40) {
          const animatedR = r * zoom;
          ctx.beginPath();
          ctx.setLineDash([10, 15]);
          ctx.arc(centerX, centerY, animatedR, 0, Math.PI * 2);
          ctx.stroke();
          
          // Lane Labels
          if (time % 2 < 0.1) {
            ctx.fillStyle = '#1e3a8a';
            ctx.font = '8px monospace';
            ctx.fillText(r < 100 ? 'RAW' : (r < 180 ? 'HOURLY' : 'DAILY'), centerX + animatedR + 5, centerY);
          }
        }
        ctx.setLineDash([]);
      }

      tenants.forEach((t, tIdx) => {
        // Add points
        if (Math.random() > 0.7) {
          t.points.push({ age: 0, initialAngle: (Math.random() - 0.5) * 0.15, size: 2 + Math.random() * 2 });
        }

        ctx.fillStyle = t.color;
        ctx.strokeStyle = t.color;
        t.points = t.points.filter(p => p.age < 6);
        t.points.forEach(p => {
          p.age += 0.012;
          // Radius grows over time, creating the spiral effect
          const r = (15 + p.age * 45) * zoom;
          // Spiral angle: time-based growth + tenant phase + global rotation
          const angle = p.age * 2.5 + t.phase + globalRotation + p.initialAngle;
          
          const x = centerX + Math.cos(angle) * r;
          const y = centerY + Math.sin(angle) * r;
          
          const currentSize = Math.max(0.5, p.size - p.age * 0.4);
          ctx.beginPath();
          ctx.arc(x, y, currentSize, 0, Math.PI * 2);
          ctx.fill();
          
          // Connect to previous position for trailing effect
          if (p.age > 0.05) {
            ctx.globalAlpha = Math.max(0, 0.4 - p.age * 0.06);
            ctx.beginPath();
            ctx.lineWidth = currentSize;
            ctx.moveTo(x, y);
            const prevR = (15 + (p.age - 0.05) * 45) * zoom;
            const prevAngle = (p.age - 0.05) * 2.5 + t.phase + globalRotation + p.initialAngle;
            ctx.lineTo(centerX + Math.cos(prevAngle) * prevR, centerY + Math.sin(prevAngle) * prevR);
            ctx.stroke();
            ctx.globalAlpha = 1.0;
          }
        });
      });

      requestAnimationFrame(draw);
    }
    
    // Simple fadeIn animation for slides
    const style = document.createElement('style');
    style.textContent = '@keyframes fadeIn { from { opacity: 0; transform: translateY(5px); } to { opacity: 1; transform: translateY(0); } }';
    document.head.appendChild(style);
    
    draw();
  });
})();
</script>

# The Geometry of Packing: Massively Multi-Tenant Scale

To understand how Spiral achieves constant-time seeks across billions of rows, we have to look at the **Packing Geometry**. By using an Archimedean spiral, we can map a 1D sequence (time) into a 2D plane where distance from the center represents "age" and the rotation phase represents "tenant identity."

In this advanced simulator, you can push the system to **100 concurrent tenants**. Watch how the **Temporal Resolution (Step)** and **Packing Density** configs mirror the internal `bundle_size` and `frame_seconds` parameters.

<div id="packing-geometry-root" class="interactive-widget" style="margin: 2rem 0; background: #020617; padding: 2rem; border-radius: 12px; border: 1px solid #1e293b; display: flex; flex-direction: column; align-items: center; min-height: 700px; position: relative;">
  <div style="width: 100%; display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 20px; flex-wrap: wrap; gap: 20px;">
    <div style="flex: 1; min-width: 300px;">
      <canvas id="packing-canvas" width="600" height="600" style="width: 100%; height: auto; background: #010409; border-radius: 8px; border: 1px solid #30363d; box-shadow: inset 0 0 20px rgba(0,0,0,0.8);"></canvas>
    </div>
    
    <div style="flex: 0 0 300px; background: #010409; padding: 1.5rem; border-radius: 10px; border: 1px solid #1e3a8a; font-family: 'JetBrains Mono', monospace; font-size: 0.75rem; box-shadow: 0 10px 30px rgba(0,0,0,0.3);">
      <div style="color: #60a5fa; margin-bottom: 15px; font-weight: bold; border-bottom: 1px solid #1e3a8a; padding-bottom: 5px; letter-spacing: 1px; font-size: 0.65rem;">POSTGRESQL_DDL.SQL</div>
      
      <div style="color: #818cf8; margin-bottom: 10px;">ALTER TABLE <span style="color: #fff;">sensor_data</span> SET (</div>
      
      <div style="margin: 0 0 15px 15px;">
        <label style="color: #94a3b8; display: block; margin-bottom: 5px; font-size: 0.65rem;">spiral.tenant = <span style="color: #fbbf24;">'sensor_id'</span>,</label>
        <div style="color: #64748b; font-size: 0.6rem; margin-bottom: 5px;">-- Scale: <span id="val-tenants" style="color: #fff;">50</span> tenants</div>
        <input type="range" id="cfg-tenants" min="1" max="100" value="50" style="width: 100%;">
      </div>

      <div style="margin: 0 0 15px 15px;">
        <label style="color: #94a3b8; display: block; margin-bottom: 5px; font-size: 0.65rem;">spiral.bundle_size = <span id="val-dist" style="color: #fbbf24;">100</span>,</label>
        <input type="range" id="cfg-dist" min="50" max="1000" step="50" value="100" style="width: 100%;">
      </div>
      
      <div style="margin: 0 0 15px 15px;">
        <label style="color: #94a3b8; display: block; margin-bottom: 5px; font-size: 0.65rem;">spiral.frames = <span id="val-width" style="color: #fbbf24;">20</span>s</label>
        <div style="color: #64748b; font-size: 0.6rem; margin-bottom: 5px;">-- Aggregation Density</div>
        <input type="range" id="cfg-width" min="10" max="50" value="20" style="width: 100%;">
      </div>

      <div style="color: #818cf8; margin-bottom: 20px;">);</div>

      <div style="color: #4ade80; margin-bottom: 15px; font-weight: bold; border-bottom: 1px solid #1e3a8a; padding-bottom: 5px; letter-spacing: 1px; font-size: 0.65rem;">QUERY_ENGINE</div>
      
      <div style="margin-bottom: 15px;">
        <label style="color: #94a3b8; display: block; margin-bottom: 5px; font-size: 0.65rem;">WHERE t >= <span id="val-tstart" style="color: #4ade80;">20%</span></label>
        <input type="range" id="cfg-tstart" min="0" max="100" value="20" style="width: 100%;">
      </div>
      
      <div>
        <label style="color: #94a3b8; display: block; margin-bottom: 5px; font-size: 0.65rem;">AND t < <span id="val-tend" style="color: #4ade80;">50%</span></label>
        <input type="range" id="cfg-tend" min="0" max="100" value="50" style="width: 100%;">
      </div>
    </div>
  </div>

  <div style="width: 100%; background: #010409; border: 1px solid #1e3a8a; padding: 12px; border-radius: 6px; font-family: 'JetBrains Mono', monospace; font-size: 0.65rem; color: #475569; display: flex; justify-content: space-between; align-items: center;">
    <div><span style="color: #4ade80;">[RUNNING]</span> <span id="packing-stats">Calculating offsets...</span></div>
    <div style="color: #1e3a8a;">ARCHIMEDEAN_V2.0_MORTON_OPT</div>
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const canvas = document.getElementById('packing-canvas');
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    
    const cfgDist = document.getElementById('cfg-dist');
    const cfgWidth = document.getElementById('cfg-width');
    const cfgTenants = document.getElementById('cfg-tenants');
    const cfgTStart = document.getElementById('cfg-tstart');
    const cfgTEnd = document.getElementById('cfg-tend');
    
    const valDist = document.getElementById('val-dist');
    const valWidth = document.getElementById('val-width');
    const valTenants = document.getElementById('val-tenants');
    const valTStart = document.getElementById('val-tstart');
    const valTEnd = document.getElementById('val-tend');
    const stats = document.getElementById('packing-stats');

    let time = 0;
    const colors = [];
    for(let i=0; i<100; i++) {
      colors.push(`hsl(${(i * 137.5) % 360}, 70%, 60%)`);
    }

    function draw() {
      const w = canvas.width;
      const h = canvas.height;
      const centerX = w / 2;
      const centerY = h / 2;
      
      ctx.clearRect(0, 0, w, h);
      time += 0.003;

      const resolution = parseInt(cfgDist.value); 
      const laneWidth = parseInt(cfgWidth.value);
      const tenantCount = parseInt(cfgTenants.value);
      const tStart = parseInt(cfgTStart.value) / 100;
      const tEnd = parseInt(cfgTEnd.value) / 100;

      valDist.textContent = resolution;
      valWidth.textContent = laneWidth;
      valTenants.textContent = tenantCount;
      valTStart.textContent = (tStart * 100).toFixed(0) + '%';
      valTEnd.textContent = (tEnd * 100).toFixed(0) + '%';

      // Draw TRUE Archimedean Spiral Grid
      ctx.strokeStyle = 'rgba(30, 41, 59, 0.6)';
      ctx.lineWidth = 1;
      ctx.setLineDash([2, 8]);
      ctx.beginPath();
      for (let a = 0; a < Math.PI * 2 * 10; a += 0.1) {
        const age = a / (Math.PI * 2 * 10);
        const r = (30 + age * (laneWidth * 8));
        const theta = a + time * 0.5;
        const x = centerX + Math.cos(theta) * r;
        const y = centerY + Math.sin(theta) * r;
        if (a === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
      }
      ctx.stroke();
      ctx.setLineDash([]);

      let highlightedPoints = 0;
      let totalPoints = 0;

      for (let t = 0; t < tenantCount; t++) {
        const color = colors[t];
        const phase = (t * Math.PI * 2) / tenantCount;
        
        // Ensure step is small enough to show a continuous spiral line
        const step = 1 / Math.max(200, resolution / 2);
        
        for (let age = 0; age < 1; age += step) {
          totalPoints++;
          const isInWindow = age >= tStart && age <= tEnd;
          
          const r = (30 + age * (laneWidth * 8));
          // Multiply age by 12 * Math.PI to ensure multiple rotations
          const theta = age * Math.PI * 6 + phase + time;
          
          const x = centerX + Math.cos(theta) * r;
          const y = centerY + Math.sin(theta) * r;
          
          if (isInWindow) {
            highlightedPoints++;
            ctx.fillStyle = color;
            if (tenantCount < 20) {
              ctx.shadowBlur = 5;
              ctx.shadowColor = color;
            }
          } else {
            ctx.fillStyle = 'rgba(30, 41, 59, 0.2)';
            ctx.shadowBlur = 0;
          }
          
          ctx.beginPath();
          const pSize = isInWindow ? (tenantCount > 50 ? 1.5 : 2.5) : (tenantCount > 50 ? 0.5 : 1);
          ctx.arc(x, y, pSize, 0, Math.PI * 2);
          ctx.fill();
        }
      }
      
      ctx.shadowBlur = 0;
      stats.innerHTML = `SCANNING [N=${tenantCount}] | BUNDLE_SIZE: ${resolution} | HITS: ${highlightedPoints} | TOTAL_OFFSETS: ${totalPoints}`;

      requestAnimationFrame(draw);
    }
    
    draw();
  });
})();
</script>

# The Engineering Problem: Entropy and Page Overhead

PostgreSQL organizes data into fixed-size **8KB Pages**. For analytical queries on massive time-series data, this creates a significant "IO Tax."

{% mermaid %}
graph TD
    subgraph "8KB Page Structure"
    H[Page Header] --> L[Line Pointers]
    L --> T1[Tuple 1]
    L --> T2[Tuple 2]
    L --> FS[Free Space]
    T1 --> ST[Special Space]
    end
{% endmermaid %}

# The B-Tree Trap in Multidimensional Queries

B-Trees are the workhorse of Postgres, but they are fundamentally one-dimensional. When you create a composite index on `(tenant_id, time)`, you are prioritizing one dimension over the other.

- **Query A** (`WHERE tenant_id = 1 AND t > ...`): **Fast**.
- **Query B** (`WHERE t > ...` across multiple tenants): **Slow**.

# Exploring Geometry-Aware Storage (Z-Ordering)

To address this, I experimented with **Z-Ordering** (the Morton Curve). It interleaves the bits of multiple dimensions to preserve locality in a multidimensional plane.

$$ \text{Z}(x, y) = \sum_{i=0}^{n-1} (x_i \cdot 2^{2i+1} + y_i \cdot 2^{2i}) $$
{: .math-formula}

# Interactive Proof of Locality

Compare how **Standard Row-Major** and **Z-Order Morton** handle memory access. Select a range and watch the **Page Loads** metric.

<div id="js-locality-root" class="interactive-widget" style="margin: 2rem 0; background: #0f172a; padding: 2rem; border-radius: 12px; border: 1px solid #1e293b; display: flex; flex-direction: column; align-items: center;">
  <div style="display: flex; gap: 2rem; width: 100%; justify-content: center; flex-wrap: wrap;">
    <div style="text-align: center;">
      <div style="font-size: 0.7rem; color: #94a3b8; margin-bottom: 5px;">STANDARD (ROW-MAJOR)</div>
      <canvas id="canvas-std" width="200" height="200" style="background: #020617; border: 1px solid #334155; cursor: crosshair;"></canvas>
      <div id="jumps-std" style="font-family: monospace; font-size: 0.7rem; color: #ef4444; margin-top: 5px;">Page Loads: 0</div>
    </div>
    <div style="text-align: center;">
      <div style="font-size: 0.7rem; color: #0ea5e9; margin-bottom: 5px;">Z-ORDER (MORTON)</div>
      <canvas id="canvas-z" width="200" height="200" style="background: #020617; border: 1px solid #334155; cursor: crosshair;"></canvas>
      <div id="jumps-z" style="font-family: monospace; font-size: 0.7rem; color: #0ea5e9; margin-top: 5px;">Page Loads: 0</div>
    </div>
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const cStd = document.getElementById('canvas-std'); const cZ = document.getElementById('canvas-z');
    const jStd = document.getElementById('jumps-std'); const jZ = document.getElementById('jumps-z');
    if (!cStd || !cZ) return;
    const ctxS = cStd.getContext('2d'); const ctxZ = cZ.getContext('2d');
    const size = 16; const cell = 200 / size; const PAGE_SIZE = 8;
    let isDrawing = false; let start = null;
    function getZ(x, y) { let z = 0; for (let i = 0; i < 4; i++) { z |= ((x & (1 << i)) << i) | ((y & (1 << i)) << (i + 1)); } return z; }
    function drawGrid(ctx) { ctx.clearRect(0,0,200,200); ctx.strokeStyle = '#1e293b'; for(let i=0; i<=size; i++) { ctx.beginPath(); ctx.moveTo(i*cell, 0); ctx.lineTo(i*cell, 200); ctx.stroke(); ctx.beginPath(); ctx.moveTo(0, i*cell); ctx.lineTo(200, i*cell); ctx.stroke(); } }
    async function visualize(x1, y1, x2, y2) {
      drawGrid(ctxS); drawGrid(ctxZ);
      const minX = Math.min(x1, x2); const maxX = Math.max(x1, x2);
      const minY = Math.min(y1, y2); const maxY = Math.max(y1, y2);
      let cells = []; for(let y=minY; y<=maxY; y++) { for(let x=minX; x<=maxX; x++) { cells.push({x, y, std: y * size + x, z: getZ(x, y)}); } }
      const stdOrder = [...cells].sort((a,b) => a.std - b.std); let activePagesS = new Set();
      for(let i=0; i<stdOrder.length; i++) {
        const p = stdOrder[i]; ctxS.fillStyle = '#ef4444'; ctxS.fillRect(p.x*cell+1, p.y*cell+1, cell-2, cell-2);
        activePagesS.add(Math.floor(p.std / PAGE_SIZE)); jStd.textContent = `Page Loads: ${activePagesS.size}`; await new Promise(r => setTimeout(r, 20));
      }
      const zOrder = [...cells].sort((a,b) => a.z - b.z); let activePagesZ = new Set();
      for(let i=0; i<zOrder.length; i++) {
        const p = zOrder[i]; ctxZ.fillStyle = '#0ea5e9'; ctxZ.fillRect(p.x*cell+1, p.y*cell+1, cell-2, cell-2);
        activePagesZ.add(Math.floor(p.z / PAGE_SIZE)); jZ.textContent = `Page Loads: ${activePagesZ.size}`; await new Promise(r => setTimeout(r, 20));
      }
    }
    const handleInput = (e) => { 
      const rect = cStd.getBoundingClientRect(); 
      const x = Math.max(0, Math.min(size-1, Math.floor((e.clientX - rect.left) / cell))); 
      const y = Math.max(0, Math.min(size-1, Math.floor((e.clientY - rect.top) / cell))); 
      if(e.type === 'mousedown') { isDrawing = true; start = {x, y}; } 
      if(e.type === 'mouseup' && isDrawing) { isDrawing = false; visualize(start.x, start.y, x, y); } 
    };
    cStd.addEventListener('mousedown', handleInput); window.addEventListener('mouseup', handleInput); drawGrid(ctxS); drawGrid(ctxZ);
  });
})();
</script>
# Prototyping a Custom Table Access Method (TAM)

PostgreSQL 12 decoupled the internal storage from the executor through the **Table Access Method (TAM)** API. This is where the research got practical. Using **Rust and `pgrx`**, I implemented a prototype handler that bypasses the standard Heap.

```rust
// Our TAM "Handshake" in src/tam.rs
pub unsafe fn spiral_tam_handler(_fcinfo: pg_sys::FunctionCallInfo) -> pgrx::datum::Internal {
    let routine = PgMemoryContexts::TopMemoryContext.palloc_struct::<pg_sys::TableAmRoutine>();
    (*routine).type_ = pg_sys::NodeTag::T_TableAmRoutine;
    
    // Wire up our experimental callbacks
    (*routine).tuple_insert = Some(spiral_slot_insert);
    (*routine).scan_begin = Some(spiral_scan_begin);
    (*routine).scan_getnextslot = Some(spiral_scan_getnextslot);
    
    pgrx::datum::Internal::from(Some(pg_sys::Datum::from(routine as usize)))
}
```

# Low-Level Experimentation: Buffer Management & O(1) Seeks

One of my experiments involved bypassing the standard heap and storing data directly into **Postgres Pages** via the Buffer Manager. By using `#[repr(C)]` in Rust, I could map data directly to hardware-aligned structures within the page and use direct offsets based on mathematical mapping:

$$ \text{PhysicalOffset} = \text{PageHeaderSize} + (\text{IndexInPage} \times \text{RowSize}) $$
{: .math-formula}

This allows for a prototype where data can be retrieved in constant time, effectively "short-circuiting" the traditional B-tree lookup while still benefiting from Postgres's crash safety and WAL logging.

```rust
// In src/storage.rs - O(1) buffer mapping
pub fn read_point(rel: Relation, t: i64, tenant_id: i64) -> Option<f64> {
    let (blkno, offset) = logical_to_physical_mapping(t, tenant_id);
    
    unsafe {
        let buffer = ReadBuffer(rel, blkno);
        LockBuffer(buffer, BUFFER_LOCK_SHARE);
        let page = BufferGetPage(buffer);
        let ptr = (page as *const u8).add(offset);
        let val = *(ptr as *const f64);
        
        UnlockReleaseBuffer(buffer);
        Some(val)
    }
}
```

# Automating the Pipeline: Hierarchical Rollups

To make the system usable, I wanted to automate the creation of hierarchies. In Spiral, a single table definition with magic comments can generate an entire analytical pipeline:

```sql
CREATE TABLE asset_ticks (
    t timestamptz NOT NULL,
    price double precision, -- Spiral: ohlc, stats, sketch
    vol int                 -- Spiral: sum
);
```

# Manual Control: Custom Hierarchies

While "Magic Comments" are great, power users can manually define rollups using standard Postgres `MATERIALIZED VIEW` syntax with Spiral options:

```sql
CREATE MATERIALIZED VIEW asset_ohlcv_1m 
WITH (
    spiral.frames = '5m,1h,1d',
    spiral.tenant = 'symbol_id'
) AS SELECT ... FROM ticks GROUP BY 1, 2;
```

# The Anatomy of a Rollup (Aggregation Inheritance)

How does Spiral know how to roll up your data? It uses deterministic rules for column inheritance:

| Column Suffix | Child Aggregate | Reason |
| :--- | :--- | :--- |
| `_h` / `_max` | `max()` | Peak is peak. |
| `_l` / `_min` | `min()` | Low is low. |
| `_sum` / `_count` | `sum()` | Accumulate totals. |
| `_sketch` | `merge()` | Mathematically unified sketches. |

# Incremental Maintenance: Tracking Changes via Changelog

Standard materialized views require a full rebuild. Spiral explores **Incremental View Maintenance (IVM)** using a transactional changelog. Every update flags a specific time bucket as "dirty."

{% mermaid %}
graph LR
    A[INSERT/UPDATE] --> B[Trigger]
    B --> C[(spiral.changelog)]
    C --> D[Background Worker]
    D --> E[Surgical Patch]
{% endmermaid %}

A background worker written in Rust monitors this log and performs surgical updates—healing the "orbits" of the spiral without rebuilding the world.

```rust
// In src/worker.rs - The Healing Loop
pub fn perform_healing() -> Result<(), pgrx::spi::Error> {
    let dirty_buckets = Spi::connect(|client| {
        client.select(
            "SELECT start_t, end_t FROM spiral.changelog WHERE processed = false",
            None, None
        )?.map(|row| (row[1].value::<i64>(), row[2].value::<i64>())).collect::<Vec<_>>()
    });

    for (start, end) in dirty_buckets {
        // Surgical rollup for this specific bucket
        rollup_bucket(start, end)?;
        
        Spi::run_with_args(
            "UPDATE spiral.changelog SET processed = true WHERE start_t = $1 AND end_t = $2",
            Some(vec![
                (PgBuiltInOids::INT8OID.oid(), start.into_datum()),
                (PgBuiltInOids::INT8OID.oid(), end.into_datum())
            ])
        )?;
    }
    Ok(())
}
```

# The Adaptive Query Slicer

One of the most complex parts of this research was exploring the **PostgreSQL Planner Hook**. The idea is to query the raw table and have the system automatically "slice" the query between different storage tiers based on data freshness and availability.

Select a time range on the timeline below, showing the last 3 days from past to present. Watch how Spiral would theoretically "slice" your query across storage tiers: **Daily**, **Hourly**, and **Minutely** rollups, with an automatic fallback to **Raw Data** for segments marked as "dirty" in the changelog. You can also simulate real-time traffic and see the background worker 'healing' the orbits.

<div id="query-slicer-root" class="interactive-widget" style="margin: 2rem 0; background: #0f172a; padding: 2rem; border-radius: 12px; border: 1px solid #1e293b; min-height: 500px;">
  <div style="margin-bottom: 2rem;">
    <div id="timeline-labels" style="display: flex; justify-content: space-between; margin-bottom: 10px; font-size: 0.7rem; color: #64748b; font-family: monospace; font-weight: bold;">
      <!-- Labels will be injected here -->
    </div>
    <div id="timeline-bg" style="height: 60px; background: #020617; border: 1px solid #334155; border-radius: 4px; position: relative; cursor: crosshair; overflow: hidden;">
       <div id="timeline-select" style="position: absolute; height: 100%; background: rgba(14, 165, 233, 0.2); border-left: 2px solid #0ea5e9; border-right: 2px solid #0ea5e9; pointer-events: none; left: 10%; width: 70%; z-index: 5;"></div>
       <div id="dirty-container" style="position: absolute; inset: 0; pointer-events: none;"></div>
       <div id="hour-markers" style="position: absolute; inset: 0; pointer-events: none; display: flex; justify-content: space-between;">
         <!-- Hour markers injected here -->
       </div>
    </div>
  </div>

  <div style="display: flex; gap: 10px; margin-bottom: 2rem; flex-wrap: wrap;">
    <button id="btn-realtime" style="background: #1e293b; color: #fff; border: 1px solid #334155; padding: 5px 12px; border-radius: 4px; font-size: 0.7rem; cursor: pointer; font-family: monospace;">+ INCOMING DATA</button>
    <button id="btn-backfill" style="background: #1e293b; color: #fff; border: 1px solid #334155; padding: 5px 12px; border-radius: 4px; font-size: 0.7rem; cursor: pointer; font-family: monospace;">⚡ RUN WORKER (BACKFILL)</button>
    <div id="sim-status" style="font-size: 0.6rem; color: #64748b; align-self: center; font-family: monospace;">STATUS: IDLE</div>
  </div>

  <div id="slice-report" style="display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 10px;"></div>

  <div style="margin-top: 2rem; padding: 1rem; background: #020617; border-radius: 6px; border: 1px solid #1e3a8a; font-family: 'JetBrains Mono', monospace; font-size: 0.7rem;">
    <div style="color: #64748b; border-bottom: 1px solid #1e3a8a; padding-bottom: 5px; margin-bottom: 10px; font-weight: bold;">PLANNER REWRITE: HIERARCHICAL UNION ALL</div>
    <div id="union-sql" style="color: #4ade80; line-height: 1.4; max-height: 150px; overflow-y: auto;"></div>
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const bg = document.getElementById('timeline-bg'); 
    const select = document.getElementById('timeline-select'); 
    const dirtyContainer = document.getElementById('dirty-container');
    const labelsRow = document.getElementById('timeline-labels');
    const hourMarkers = document.getElementById('hour-markers');
    const report = document.getElementById('slice-report'); 
    const sql = document.getElementById('union-sql');
    const btnRealtime = document.getElementById('btn-realtime');
    const btnBackfill = document.getElementById('btn-backfill');
    const simStatus = document.getElementById('sim-status');

    if (!bg) return;

    let isDragging = false; 
    let startX = 10; 
    let currentSelect = { left: 10, width: 70 };
    let dirtyRanges = [{s: 31, e: 35}, {s: 90, e: 98}]; 
    
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const startDate = new Date(today.getTime() - 2 * 24 * 60 * 60 * 1000);

    const formatDate = (d) => d.toISOString().split('T')[0];
    const formatFull = (d) => d.toISOString().replace('T', ' ').split('.')[0];

    // Initialize Labels
    labelsRow.innerHTML = [0, 1, 2].map(i => {
      const d = new Date(startDate.getTime() + i * 24 * 60 * 60 * 1000);
      return `<span>${formatDate(d)}</span>`;
    }).join('');
    
    // Initialize Hour Markers & Rule
    for (let i = 0; i <= 72; i += 3) {
      const marker = document.createElement('div');
      marker.style.width = '1px';
      marker.style.height = i % 24 === 0 ? '100%' : (i % 6 === 0 ? '40%' : '15%');
      marker.style.background = i % 24 === 0 ? '#334155' : (i % 6 === 0 ? '#1e3a8a' : '#1e293b');
      marker.style.position = 'relative';
      
      if (i % 6 === 0 && i < 72) {
        const hLabel = document.createElement('span');
        hLabel.textContent = (i % 24).toString().padStart(2, '0') + 'h';
        hLabel.style.position = 'absolute';
        hLabel.style.top = '100%';
        hLabel.style.left = '0';
        hLabel.style.fontSize = '0.5rem';
        hLabel.style.color = '#475569';
        hLabel.style.transform = 'translateX(-50%)';
        marker.appendChild(hLabel);
      }
      hourMarkers.appendChild(marker);
    }

    function renderDirty() {
      dirtyContainer.innerHTML = '';
      dirtyRanges.forEach(range => {
        const div = document.createElement('div');
        div.style.position = 'absolute';
        div.style.height = '100%';
        div.style.left = range.s + '%';
        div.style.width = (range.e - range.s) + '%';
        div.style.background = 'repeating-linear-gradient(45deg, transparent, transparent 5px, rgba(239, 68, 68, 0.3) 5px, rgba(239, 68, 68, 0.3) 10px)';
        div.style.borderLeft = '1px solid #ef4444';
        div.style.borderRight = '1px solid #ef4444';
        dirtyContainer.appendChild(div);
      });
    }

    function updateSlices() {
      const { left, width } = currentSelect;
      const startP = left; const endP = left + width; 
      report.innerHTML = '';
      let sqlParts = [];

      const totalMs = 3 * 24 * 60 * 60 * 1000;
      const globalStart = startDate.getTime();
      const queryStart = globalStart + (startP / 100) * totalMs;
      const queryEnd = globalStart + (endP / 100) * totalMs;

      if (width <= 0.01) {
        sql.innerHTML = '-- No range selected';
        return;
      }

      const TIERS = [
        { name: 'DAILY ROLLUP', table: 'ticks_1d', ms: 24 * 60 * 60 * 1000, color: '#10b981' },
        { name: 'HOURLY ROLLUP', table: 'ticks_1h', ms: 60 * 60 * 1000, color: '#0ea5e9' },
        { name: 'MINUTELY ROLLUP', table: 'ticks_1m', ms: 60 * 1000, color: '#818cf8' },
        { name: 'RAW DATA', table: 'raw_ticks', ms: 1000, color: '#ef4444' }
      ];

      let segments = [];
      let current = queryStart;

      while (current < queryEnd - 500) {
        let bestTier = TIERS[3]; // Default to Raw
        let sliceEnd = queryEnd;

        const currentP = ((current - globalStart) / totalMs) * 100;
        let dirtyIdx = dirtyRanges.findIndex(d => currentP < d.e && (currentP + 0.1) > d.s);
        
        if (dirtyIdx !== -1) {
          bestTier = TIERS[3];
          const dirtyEndMs = globalStart + (dirtyRanges[dirtyIdx].e / 100) * totalMs;
          sliceEnd = Math.min(queryEnd, dirtyEndMs);
        } else {
          for (let tier of TIERS) {
            const isAligned = Math.floor(current / tier.ms) * tier.ms === current;
            const nextAlignment = (Math.floor(current / tier.ms) + 1) * tier.ms;
            
            if (isAligned && nextAlignment <= queryEnd) {
              const nextP = ((nextAlignment - globalStart) / totalMs) * 100;
              const hasDirty = dirtyRanges.some(d => d.s < nextP && d.e > currentP);
              if (!hasDirty) {
                bestTier = tier;
                sliceEnd = nextAlignment;
                break;
              }
            }
          }
          if (bestTier.table === 'raw_ticks') {
            const nextMin = (Math.floor(current / 60000) + 1) * 60000;
            const nextDirty = dirtyRanges.find(d => d.s > currentP);
            const nextDirtyMs = nextDirty ? globalStart + (nextDirty.s / 100) * totalMs : queryEnd;
            sliceEnd = Math.min(queryEnd, nextMin, nextDirtyMs);
          }
        }
        segments.push({ tier: bestTier, start: current, end: sliceEnd });
        current = sliceEnd;
      }

      // Normalize: Merge contiguous segments of the same tier
      let normalized = [];
      if (segments.length > 0) {
        let last = segments[0];
        for (let i = 1; i < segments.length; i++) {
          if (segments[i].tier.table === last.tier.table) {
            last.end = segments[i].end;
          } else {
            normalized.push(last);
            last = segments[i];
          }
        }
        normalized.push(last);
      }

      // Group normalized ranges by tier and sort Daily -> Raw (climb and descend)
      const groups = TIERS.map(t => ({
        tier: t,
        ranges: normalized.filter(s => s.tier.table === t.table)
      }))
      .filter(g => g.ranges.length > 0)
      .sort((a, b) => b.tier.ms - a.tier.ms);

      let cteParts = [];
      groups.forEach((group, idx) => {
        const div = document.createElement('div'); 
        div.style.borderLeft = `4px solid ${group.tier.color}`; 
        div.style.background = 'rgba(255,255,255,0.03)'; 
        div.style.padding = '8px';
        div.innerHTML = `<div style="font-family: monospace; font-size: 0.6rem; color: #94a3b8;">LEVEL: ${group.tier.name}</div>
                         <div style="font-family: monospace; font-size: 0.7rem; color: #fff;">MULTIRANGE ACCELERATION: ${group.ranges.length} BIN(S)</div>`;
        report.appendChild(div);

        const scanName = group.tier.table + '_scan';
        const multirange = '{' + group.ranges.map(r => 
          `[${formatFull(new Date(r.start))}, ${formatFull(new Date(r.end))})`
        ).join(', ') + '}';

        cteParts.push(`<span style="color: #64748b;">  ${scanName} AS (</span><br>    SELECT * FROM ${group.tier.table} <br>    WHERE t <span style="color: #818cf8;"><@</span> <span style="color: #fbbf24;">'${multirange}'</span>::tstzmultirange<br><span style="color: #64748b;">  )</span>`);
      });

      if (cteParts.length > 0) {
        let finalSql = `<span style="color: #64748b;">-- Optimized Hierarchical Query Plan</span><br><span style="color: #818cf8;">WITH</span><br>${cteParts.join(',<br>')}<br>`;
        finalSql += groups.map(g => `<span style="color: #818cf8;">SELECT</span> * <span style="color: #818cf8;">FROM</span> ${g.tier.table}_scan`).join('<br><span style="color: #334155; font-weight: bold;">UNION ALL</span><br>');
        finalSql += `<br><span style="color: #818cf8;">ORDER BY</span> t; <span style="color: #64748b;">-- Ensure chronological consistency</span>`;
        sql.innerHTML = finalSql;
      }
    }

    bg.addEventListener('mousedown', (e) => { 
      isDragging = true; 
      const rect = bg.getBoundingClientRect();
      startX = ((e.clientX - rect.left) / rect.width) * 100; 
      currentSelect = { left: startX, width: 0 };
      select.style.left = startX + '%';
      select.style.width = '0%'; 
    });

    window.addEventListener('mousemove', (e) => { 
      if (!isDragging) return; 
      const rect = bg.getBoundingClientRect();
      let cur = ((e.clientX - rect.left) / rect.width) * 100; 
      cur = Math.max(0, Math.min(100, cur)); 
      const left = Math.min(startX, cur); 
      const width = Math.abs(cur - startX); 
      currentSelect = { left, width };
      select.style.left = left + '%'; 
      select.style.width = width + '%'; 
      updateSlices(); 
    });

    window.addEventListener('mouseup', () => { isDragging = false; });

    btnRealtime.addEventListener('click', () => {
      simStatus.textContent = 'STATUS: RECEIVING TRAFFIC...';
      const last = dirtyRanges[dirtyRanges.length-1];
      if (last && last.e > 95) {
        last.e = 100;
      } else {
        dirtyRanges.push({s: 98, e: 100});
      }
      renderDirty();
      updateSlices();
      setTimeout(() => simStatus.textContent = 'STATUS: IDLE', 1000);
    });

    btnBackfill.addEventListener('click', () => {
      if (dirtyRanges.length === 0) return;
      simStatus.textContent = 'STATUS: WORKER PROCESSING...';
      
      const interval = setInterval(() => {
        let changed = false;
        dirtyRanges = dirtyRanges.map(r => {
          if (r.e - r.s > 1) {
            changed = true;
            return { s: r.s + 1, e: r.e };
          }
          return null;
        }).filter(Boolean);

        renderDirty();
        updateSlices();

        if (!changed || dirtyRanges.length === 0) {
          clearInterval(interval);
          simStatus.textContent = 'STATUS: ALL ORBITS HEALED';
          setTimeout(() => simStatus.textContent = 'STATUS: IDLE', 2000);
        }
      }, 100);
    });

    renderDirty();
    updateSlices();
  });
})();
</script>

# Live Demo: The Modern Spiral Setup

Let's see this in action using PostgreSQL's native `WITH` syntax.

```sql
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- The WITH clause tells Spiral to track this table,
-- set up specific rollups, and isolate data by 'sensor_id'.
CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision, -- Spiral: ohlcv
    humidity double precision,    -- Spiral: sum
    power_usage double precision  -- Spiral: stats
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'sensor_id'
);
```

# Behind the Scenes: Auto-created Views

Spiral immediately registers the table and creates the hierarchical views.

```sql
SELECT view_name, parent_view, frame_seconds, scope_columns 
FROM spiral.metadata ORDER BY frame_seconds;
```

```text
   view_name    |  parent_view   | frame_seconds | scope_columns 
----------------+----------------+---------------+---------------
 sensor_data_1m | sensor_data    |            60 | sensor_id
 sensor_data_1h | sensor_data_1m |          3600 | sensor_id
```

Notice it used the magic comments to build the 1m and 1h schema dynamically!

# Data Ingestion spanning Timeframes

Let's insert some raw data.

```sql
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage) VALUES
('2026-05-03 20:15:00'::timestamptz, 1, 22.5, 45.0, 100.5),
('2026-05-03 20:15:00'::timestamptz, 1, 22.7, 45.2, 101.0),
('2026-05-03 20:15:00'::timestamptz, 2, 19.5, 50.0, 80.0),
('2026-05-03 20:16:00'::timestamptz, 1, 23.0, 44.0, 105.0),
('2026-05-03 20:16:00'::timestamptz, 2, 19.8, 51.0, 82.0),
('2026-05-03 21:15:00'::timestamptz, 1, 25.0, 40.0, 110.0);
```

# Incremental Cascading Refresh

Refreshing the 1-minute rollup incrementally processes the new raw data.

```sql
SELECT spiral_refresh('sensor_data');
```

The column names are dynamically derived from the magic comments:

```sql
SELECT t, sensor_id, 
       temperature_ohlcv_o, temperature_ohlcv_h, 
       temperature_ohlcv_l, temperature_ohlcv_c,
       humidity, power_usage_stats
FROM sensor_data_1m ORDER BY t, sensor_id;
```

Refresh cascades to the 1-hour rollup automatically because Spiral knows the hierarchy!

# Transparent Query Acceleration

Spiral intercepts queries to the base table and rewrites them to use the pre-aggregated rollups seamlessly!

```sql
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz 
  AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;
```

*The query hits `sensor_data_1h` or `sensor_data_1m` depending on the requested grouping.*

# Handling Late-Arriving Data

Let's insert an hour of randomized, late data for all tenants.

```sql
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage)
SELECT 
    '2026-05-03 22:00:00'::timestamptz + (random() * 60 || ' minutes')::interval,
    id,
    20 + random() * 10,
    40 + random() * 20,
    90 + random() * 30
FROM generate_series(1, 2) AS id, generate_series(1, 60);
```

Spiral tracks exactly which time buckets and tenants are "dirty".

```sql
SELECT base_view, t_start, t_end, scope_values 
FROM spiral.changelog ORDER BY t_start;
```

# Smart Query Slicing in Action

Spiral's planner is smart enough to know about the dirty data. It slices the query: clean segments go to the rollup, dirty segments go to the raw table!

```sql
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz 
  AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;
```

# Healing the Orbits & Handling Deletions

We heal the dirty buckets. It only processes the changes!

```sql
SELECT spiral_refresh('sensor_data');
```

The entire time range is now clean, so it uses 100% rollups! If we delete data, it isolates the dirty fallback to ONLY the affected tenant (`sensor_id = 1`) for that specific time bucket.

```sql
DELETE FROM sensor_data 
WHERE sensor_id = 1 
  AND t >= '2026-05-03 20:15:00'::timestamptz 
  AND t < '2026-05-03 20:25:00'::timestamptz;
```

# Mathematical Engine: Welford’s Algorithm

How do we "merge" a standard deviation? Spiral uses **Welford's Algorithm** to store statistical moments.

$$ \bar{x}_n = \bar{x}_{n-1} + \frac{x_n - \bar{x}_{n-1}}{n} $$
{: .math-formula}

$$ M_{2,n} = M_{2,n-1} + (x_n - \bar{x}_{n-1})(x_n - \bar{x}_n) $$
{: .math-formula}

# Experimental Results: Prototype Benchmarks

In my local environment, with a **10 million row dataset**, I saw results that justified continuing this research.

| Phase | Ingest Rate | Duration |
| :--- | :--- | :--- |
| **Bulk Load** | 1,940,482 rows/s | 0.51s (1M sample) |
| **Backfill** | 302,413 rows/s | 3.30s (1M sample) |

# Open for Collaboration

Building Spiral has been a rewarding learning experience. This is a **work in progress**. If you are a PostgreSQL internals expert, a Rustacean, or someone who loves high-performance storage, I invite you to contribute.

---
*Spiral is open source. [Check it out on GitHub](https://github.com/jonatas/spiral) and let's explore the future of storage together.*
