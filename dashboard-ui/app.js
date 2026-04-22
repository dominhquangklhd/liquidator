let profitChart;
let statusChart;
let sqliteDb = null;
let hasStatusColumn = false;
let tableColumns = {
  liquidations: new Set(),
  profit: new Set(),
};

const dbFileInput = document.getElementById("dbFile");
const hoursSelect = document.getElementById("hours");
const refreshBtn = document.getElementById("refreshBtn");
const dbMeta = document.getElementById("dbMeta");

const kpiAttempts = document.getElementById("kpiAttempts");
const kpiSuccessRate = document.getElementById("kpiSuccessRate");
const kpiNetProfit = document.getElementById("kpiNetProfit");
const kpiGasCost = document.getElementById("kpiGasCost");

function formatUsd(value) {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2,
  }).format(value || 0);
}

function formatShortAddress(address) {
  if (!address || address.length < 12) return address || "-";
  return `${address.slice(0, 6)}...${address.slice(-4)}`;
}

function formatTime(ts) {
  if (!ts) return "-";
  return new Date(ts * 1000).toLocaleString("vi-VN", { hour12: false });
}

function rowsFromQuery(sql, params = []) {
  if (!sqliteDb) {
    return [];
  }

  const result = sqliteDb.exec(sql, params);
  if (!result.length) {
    return [];
  }

  const { columns, values } = result[0];
  return values.map((valueRow) => {
    const row = {};
    for (let idx = 0; idx < columns.length; idx += 1) {
      row[columns[idx]] = valueRow[idx];
    }
    return row;
  });
}

function getCurrentUnixSeconds() {
  return Math.floor(Date.now() / 1000);
}

function tableHasColumn(tableName, columnName) {
  const cols = tableColumns[tableName];
  if (!cols) {
    return false;
  }
  return cols.has(columnName.toLowerCase());
}

function columnOrLiteral(tableName, columnName, literalSql) {
  if (tableHasColumn(tableName, columnName)) {
    return columnName;
  }
  return literalSql;
}

function getTableColumns(tableName) {
  const rows = rowsFromQuery(`PRAGMA table_info(${tableName})`);
  const cols = new Set();
  for (const row of rows) {
    const colName = String(row.name || "").toLowerCase();
    if (colName) {
      cols.add(colName);
    }
  }
  return cols;
}

function getStatusExpression() {
  if (hasStatusColumn) {
    return "COALESCE(status, 'unknown')";
  }
  if (tableHasColumn("liquidations", "error_message")) {
    return "CASE WHEN error_message IS NULL THEN 'success' ELSE 'failed' END";
  }
  return "'unknown'";
}

function getSummary(hours) {
  const since = getCurrentUnixSeconds() - (hours * 3600);
  const statusExpr = getStatusExpression();
  const profitExpr = `COALESCE(${columnOrLiteral("liquidations", "profit_usd", "0")}, 0)`;
  const gasExpr = `COALESCE(${columnOrLiteral("liquidations", "gas_cost_usd", "0")}, 0)`;

  const rows = rowsFromQuery(
    `
      SELECT
        COUNT(*) AS total_attempts,
        SUM(CASE WHEN ${statusExpr} = 'success' THEN 1 ELSE 0 END) AS success_count,
        SUM(${profitExpr}) AS total_profit_usd,
        SUM(${gasExpr}) AS total_gas_cost_usd
      FROM liquidations
      WHERE timestamp >= ?
    `,
    [since],
  );

  const row = rows[0] || {};
  const totalAttempts = Number(row.total_attempts || 0);
  const successCount = Number(row.success_count || 0);
  const successRate = totalAttempts ? (successCount * 100) / totalAttempts : 0;
  const totalProfit = Number(row.total_profit_usd || 0);
  const totalGas = Number(row.total_gas_cost_usd || 0);

  return {
    total_attempts: totalAttempts,
    success_rate_pct: successRate,
    net_profit_usd: totalProfit - totalGas,
    total_gas_cost_usd: totalGas,
  };
}

function getProfitTimeseries(hours) {
  const since = getCurrentUnixSeconds() - (hours * 3600);
  const profitExpr = `COALESCE(${columnOrLiteral("liquidations", "profit_usd", "0")}, 0)`;
  const gasExpr = `COALESCE(${columnOrLiteral("liquidations", "gas_cost_usd", "0")}, 0)`;
  return rowsFromQuery(
    `
      SELECT
        CAST((timestamp / 3600) AS INTEGER) * 3600 AS bucket_ts,
        SUM(${profitExpr} - ${gasExpr}) AS net_profit_usd,
        SUM(${gasExpr}) AS total_gas_cost_usd
      FROM liquidations
      WHERE timestamp >= ?
      GROUP BY bucket_ts
      ORDER BY bucket_ts ASC
    `,
    [since],
  );
}

function getStatusBreakdown(hours) {
  const since = getCurrentUnixSeconds() - (hours * 3600);
  const statusExpr = getStatusExpression();
  return rowsFromQuery(
    `
      SELECT
        ${statusExpr} AS status,
        COUNT(*) AS count
      FROM liquidations
      WHERE timestamp >= ?
      GROUP BY status
      ORDER BY count DESC
    `,
    [since],
  );
}

function getRecentLiquidations(limit = 20) {
  const statusExpr = getStatusExpression();
  const userExpr = columnOrLiteral("liquidations", "user_address", "'-'");
  const collateralExpr = columnOrLiteral("liquidations", "collateral_asset", "'-'");
  const debtExpr = columnOrLiteral("liquidations", "debt_asset", "'-'");
  const profitExpr = columnOrLiteral("liquidations", "profit_usd", "0");
  const gasExpr = columnOrLiteral("liquidations", "gas_cost_usd", "0");

  return rowsFromQuery(
    `
      SELECT
        timestamp,
        ${userExpr} AS user_address,
        ${collateralExpr} AS collateral_asset,
        ${debtExpr} AS debt_asset,
        ${profitExpr} AS profit_usd,
        ${gasExpr} AS gas_cost_usd,
        ${statusExpr} AS status
      FROM liquidations
      ORDER BY timestamp DESC
      LIMIT ?
    `,
    [limit],
  );
}

function inspectSchema() {
  const tableExists = rowsFromQuery(
    "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'liquidations' LIMIT 1",
  );

  if (!tableExists.length) {
    throw new Error("Khong tim thay bang liquidations trong file DB.");
  }

  tableColumns = {
    liquidations: getTableColumns("liquidations"),
    profit: getTableColumns("profit"),
  };

  hasStatusColumn = tableHasColumn("liquidations", "status");

  if (!tableHasColumn("liquidations", "timestamp")) {
    throw new Error("Bang liquidations khong co cot timestamp.");
  }
}

async function loadDbFile(file) {
  const SQL = await initSqlJs({
    locateFile: (name) => `https://cdn.jsdelivr.net/npm/sql.js@1.12.0/dist/${name}`,
  });
  const bytes = new Uint8Array(await file.arrayBuffer());

  if (sqliteDb) {
    sqliteDb.close();
  }

  sqliteDb = new SQL.Database(bytes);
  inspectSchema();

  dbMeta.textContent = `Da nap DB: ${file.name} | Kich thuoc: ${(file.size / (1024 * 1024)).toFixed(2)} MB`;
}

function updateKpis(summary) {
  kpiAttempts.textContent = String(summary.total_attempts ?? 0);
  kpiSuccessRate.textContent = `${(summary.success_rate_pct ?? 0).toFixed(1)}%`;
  kpiNetProfit.textContent = formatUsd(summary.net_profit_usd ?? 0);
  kpiGasCost.textContent = formatUsd(summary.total_gas_cost_usd ?? 0);
}

function drawNoData(canvas, message) {
  const ctx = canvas.getContext("2d");
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.save();
  ctx.fillStyle = "#57534e";
  ctx.font = "600 14px Space Grotesk, sans-serif";
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(message, canvas.width / 2, canvas.height / 2);
  ctx.restore();
}

function renderProfitChart(points) {
  const canvas = document.getElementById("profitChart");
  const labels = points.map((p) => formatTime(p.bucket_ts));
  const netProfit = points.map((p) => Number(p.net_profit_usd || 0));
  const gasCost = points.map((p) => Number(p.total_gas_cost_usd || 0));

  if (profitChart) profitChart.destroy();
  if (!points.length) {
    drawNoData(canvas, "Khong co du lieu cho khung thoi gian da chon");
    return;
  }

  profitChart = new Chart(canvas, {
    type: "line",
    data: {
      labels,
      datasets: [
        {
          label: "Net Profit (USD)",
          data: netProfit,
          borderColor: "#e36414",
          backgroundColor: "rgba(227,100,20,0.15)",
          fill: true,
          tension: 0.24,
        },
        {
          label: "Gas Cost (USD)",
          data: gasCost,
          borderColor: "#2b6cb0",
          backgroundColor: "rgba(43,108,176,0.10)",
          fill: false,
          tension: 0.18,
        },
      ],
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      animation: false,
      scales: {
        x: {
          ticks: {
            autoSkip: true,
            maxTicksLimit: 12,
          },
        },
      },
      plugins: {
        legend: { position: "bottom" },
      },
    },
  });
}

function renderStatusChart(statusRows) {
  const canvas = document.getElementById("statusChart");
  const labels = statusRows.map((s) => s.status || "unknown");
  const values = statusRows.map((s) => Number(s.count || 0));

  if (statusChart) statusChart.destroy();
  if (!statusRows.length) {
    drawNoData(canvas, "Khong co du lieu trang thai");
    return;
  }

  statusChart = new Chart(canvas, {
    type: "doughnut",
    data: {
      labels,
      datasets: [
        {
          data: values,
          backgroundColor: ["#2f855a", "#c53030", "#2b6cb0", "#d69e2e", "#4a5568"],
        },
      ],
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      animation: false,
      plugins: {
        legend: { position: "bottom" },
      },
    },
  });
}

function renderRecentTable(rows) {
  const tbody = document.getElementById("recentTableBody");
  tbody.innerHTML = "";

  if (!rows.length) {
    const tr = document.createElement("tr");
    tr.innerHTML = '<td colspan="7">Khong co liquidation nao trong DB.</td>';
    tbody.appendChild(tr);
    return;
  }

  for (const row of rows) {
    const tr = document.createElement("tr");
    const net = (row.profit_usd || 0) - (row.gas_cost_usd || 0);
    const statusClass = row.status === "success" ? "status-success" : "status-failed";

    tr.innerHTML = `
      <td>${formatTime(row.timestamp)}</td>
      <td>${formatShortAddress(row.user_address)}</td>
      <td>${row.collateral_asset}/${row.debt_asset}</td>
      <td>${formatUsd(row.profit_usd)}</td>
      <td>${formatUsd(row.gas_cost_usd)}</td>
      <td>${formatUsd(net)}</td>
      <td><span class="status-pill ${statusClass}">${row.status}</span></td>
    `;

    tbody.appendChild(tr);
  }
}

async function refreshDashboard() {
  if (!sqliteDb) {
    alert("Hay nap file .db truoc khi refresh.");
    return;
  }

  const hours = Number(hoursSelect.value || 24);

  refreshBtn.disabled = true;
  refreshBtn.textContent = "Loading...";

  try {
    const summary = getSummary(hours);
    const timeseries = getProfitTimeseries(hours);
    const statusRows = getStatusBreakdown(hours);
    const recentRows = getRecentLiquidations(20);

    updateKpis(summary);
    renderProfitChart(timeseries);
    renderStatusChart(statusRows);
    renderRecentTable(recentRows);
    dbMeta.textContent = `${dbMeta.textContent.split(" | ")[0]} | ${summary.total_attempts} attempts trong ${hours}h`;
  } catch (error) {
    console.error(error);
    alert("Khong the tai du lieu dashboard tu file DB. Kiem tra schema bang liquidations.");
  } finally {
    refreshBtn.disabled = false;
    refreshBtn.textContent = "Refresh";
  }
}

async function onDbSelected(event) {
  const [file] = event.target.files || [];
  if (!file) {
    return;
  }

  refreshBtn.disabled = true;
  dbMeta.textContent = "Dang nap file DB...";

  try {
    await loadDbFile(file);
    await refreshDashboard();
  } catch (error) {
    console.error(error);
    sqliteDb = null;
    dbMeta.textContent = "Nap file DB that bai.";
    alert("Nap file SQLite that bai. Hay chon dung file DB cua module storage.");
  } finally {
    refreshBtn.disabled = false;
  }
}

dbFileInput.addEventListener("change", onDbSelected);
hoursSelect.addEventListener("change", refreshDashboard);
refreshBtn.addEventListener("click", refreshDashboard);
