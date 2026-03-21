#!/bin/bash
#
# ============================================================================
# SCRIPTS-UBUNTU - Bash versions of liquidator setup scripts
# ============================================================================
#
# Thay thế PowerShell scripts bằng Bash để sử dụng trên Linux/macOS/WSL
#
# Setup:
#   1. chmod +x scripts-ubuntu/*.sh              # Make executable
#   2. Tao/cap nhat file .env o root project voi dong:
#      ETH_RPC_URL=https://eth-mainnet.g.alchemy.com/v2/YOUR_KEY
#   3. ./scripts-ubuntu/start_anvil.sh            # Start fork
#   4. ./scripts-ubuntu/setup_liquidation_scenario.sh  # Setup scenario
#   5. ./scripts-ubuntu/crash_price.sh -d 25     # Crash price
#   6. cargo run                                 # Run liquidator
#
# Luu y:
#   - Cac script .sh da tu dong load .env o root project.
#   - Ban van co the truyen -r/--rpc-url de override tam thoi.
#
# Available scripts:
#   - start_anvil.sh: Start Anvil fork
#   - setup_liquidation_scenario.sh: Create liquidatable position
#   - crash_price.sh: Crash ETH price to trigger liquidation
#
# Examples:
#   ./scripts-ubuntu/start_anvil.sh -n sepolia -p 8545
#   ./scripts-ubuntu/crash_price.sh -d 30
#   ./scripts-ubuntu/crash_price.sh -r http://localhost:8545 -d 25
#
# Made executable:
