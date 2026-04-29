
# Locality in the Browser: JS Implementation

Wait, is this just for database gurus? **No.** The same math that speeds up Spiral can revolutionize how you handle large datasets in the frontend—think Canvas rendering, Map clustering, or complex WebGL state.

```javascript
// The Morton Curve in 5 lines of JS
const getZ = (x, y) => {
  let z = 0;
  for (let i = 0; i < 16; i++) {
    z |= (x & (1 << i)) << i | (y & (1 << i)) << (i + 1);
  }
  return z >>> 0; // Return as unsigned 32-bit
};
```

By applying this in JavaScript, we can keep conceptually "nearby" points close in a flat `Float32Array`, maximizing CPU cache hits even in the browser.

# Standard vs. Z-Order: The Frontend Scan

Select a range on the grid below. Watch the **Array Traversal Path**. Notice how the Standard sort (Row-major) forces huge "jumps" in memory, while Z-Order keeps the access pattern tightly clustered.

<div id="js-locality-root" class="interactive-widget" style="margin: 2rem 0; background: #0f172a; padding: 2rem; border-radius: 12px; border: 1px solid #1e293b; display: flex; flex-direction: column; align-items: center;">
  <div style="display: flex; gap: 2rem; width: 100%; justify-content: center;">
    <div style="text-align: center;">
      <div style="font-size: 0.7rem; color: #94a3b8; margin-bottom: 5px;">STANDARD ARRAY ACCESS</div>
      <canvas id="canvas-std" width="200" height="200" style="background: #020617; border: 1px solid #334155; cursor: crosshair;"></canvas>
      <div id="jumps-std" style="font-family: monospace; font-size: 0.7rem; color: #ef4444; margin-top: 5px;">Jumps: 0</div>
    </div>
    <div style="text-align: center;">
      <div style="font-size: 0.7rem; color: #0ea5e9; margin-bottom: 5px;">Z-ORDER ARRAY ACCESS</div>
      <canvas id="canvas-z" width="200" height="200" style="background: #020617; border: 1px solid #334155; cursor: crosshair;"></canvas>
      <div id="jumps-z" style="font-family: monospace; font-size: 0.7rem; color: #0ea5e9; margin-top: 5px;">Jumps: 0</div>
    </div>
  </div>
  <div style="margin-top: 1rem; color: #64748b; font-size: 0.8rem; text-align: center;">
    Click and drag to select a range!
  </div>
</div>

<script>
(function() {
  document.addEventListener('DOMContentLoaded', function() {
    const cStd = document.getElementById('canvas-std');
    const cZ = document.getElementById('canvas-z');
    const jStd = document.getElementById('jumps-std');
    const jZ = document.getElementById('jumps-z');
    if (!cStd || !cZ) return;

    const ctxS = cStd.getContext('2d');
    const ctxZ = cZ.getContext('2d');
    const size = 16;
    const cell = 200 / size;

    let isDrawing = false;
    let start = null;

    function getZ(x, y) {
      let z = 0;
      for (let i = 0; i < 4; i++) { z |= ((x & (1 << i)) << i) | ((y & (1 << i)) << (i + 1)); }
      return z;
    }

    function drawGrid(ctx) {
      ctx.clearRect(0,0,200,200);
      ctx.strokeStyle = '#1e293b';
      for(let i=0; i<=size; i++) {
        ctx.beginPath(); ctx.moveTo(i*cell, 0); ctx.lineTo(i*cell, 200); ctx.stroke();
        ctx.beginPath(); ctx.moveTo(0, i*cell); ctx.lineTo(200, i*cell); ctx.stroke();
      }
    }

    async function visualize(x1, y1, x2, y2) {
      drawGrid(ctxS); drawGrid(ctxZ);
      const minX = Math.min(x1, x2); const maxX = Math.max(x1, x2);
      const minY = Math.min(y1, y2); const maxY = Math.max(y1, y2);

      // Collect cells
      let cells = [];
      for(let y=minY; y<=maxY; y++) {
        for(let x=minX; x<=maxX; x++) {
          cells.push({x, y, std: y * size + x, z: getZ(x, y)});
        }
      }

      // Standard Scan
      const stdOrder = [...cells].sort((a,b) => a.std - b.std);
      let sj = 0;
      for(let i=0; i<stdOrder.length; i++) {
        const p = stdOrder[i];
        ctxS.fillStyle = '#ef4444'; ctxS.fillRect(p.x*cell+1, p.y*cell+1, cell-2, cell-2);
        if(i > 0 && Math.abs(stdOrder[i].std - stdOrder[i-1].std) > 1) sj++;
        jStd.textContent = `Memory Jumps: ${sj}`;
        await new Promise(r => setTimeout(r, 20));
      }

      // Z-Order Scan
      const zOrder = [...cells].sort((a,b) => a.z - b.z);
      let zj = 0;
      for(let i=0; i<zOrder.length; i++) {
        const p = zOrder[i];
        ctxZ.fillStyle = '#0ea5e9'; ctxZ.fillRect(p.x*cell+1, p.y*cell+1, cell-2, cell-2);
        if(i > 0 && Math.abs(zOrder[i].z - zOrder[i-1].z) > 1) zj++;
        jZ.textContent = `Memory Jumps: ${zj}`;
        await new Promise(r => setTimeout(r, 20));
      }
    }

    const handleInput = (e) => {
      const rect = cStd.getBoundingClientRect();
      const x = Math.floor((e.clientX - rect.left) / cell);
      const y = Math.floor((e.clientY - rect.top) / cell);
      if(e.type === 'mousedown') { isDrawing = true; start = {x, y}; }
      if(e.type === 'mouseup' && isDrawing) { isDrawing = false; visualize(start.x, start.y, x, y); }
    };

    cStd.addEventListener('mousedown', handleInput);
    window.addEventListener('mouseup', handleInput);
    drawGrid(ctxS); drawGrid(ctxZ);
  });
})();
</script>
