// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

/// @title MockPriceFeed
/// @notice Mock implementation của Chainlink AggregatorV3Interface
/// @dev Dùng để test liquidation trên local Anvil fork
///      Cho phép thay đổi giá tùy ý để trigger liquidation
///
/// Deploy và sử dụng:
///   forge create contracts/MockPriceFeed.sol:MockPriceFeed \
///     --constructor-args <initial_price_8_decimals> \
///     --rpc-url http://127.0.0.1:8545 \
///     --unlocked --from <aave_oracle_owner>
///
///   # Thay đổi giá:
///   cast send <mock_address> "setAnswer(int256)" <new_price> \
///     --rpc-url http://127.0.0.1:8545 \
///     --unlocked --from <any_account>
///
/// Ví dụ giá ETH:
///   $2500 → 250000000000 (2500 * 1e8)
///   $1800 → 180000000000 (1800 * 1e8)  
///   $1200 → 120000000000 (1200 * 1e8) ← sẽ trigger liquidation

contract MockPriceFeed {
    int256 private _answer;
    uint8 private _decimals;
    string private _description;
    uint256 private _version;
    uint80 private _roundId;
    uint256 private _updatedAt;

    event AnswerUpdated(int256 indexed current, uint256 indexed roundId, uint256 updatedAt);

    constructor(int256 initialAnswer) {
        _answer = initialAnswer;
        _decimals = 8;
        _description = "Mock ETH/USD";
        _version = 1;
        _roundId = 1;
        _updatedAt = block.timestamp;
    }

    /// @notice Thay đổi giá - đây là hàm chính để test
    /// @param newAnswer Giá mới (8 decimals). VD: 2500 USD = 250000000000
    function setAnswer(int256 newAnswer) external {
        _answer = newAnswer;
        _roundId++;
        _updatedAt = block.timestamp;
        emit AnswerUpdated(newAnswer, _roundId, block.timestamp);
    }

    /// @notice Giảm giá theo phần trăm
    /// @param percentDrop Phần trăm giảm (0-100). VD: 30 = giảm 30%
    function dropPrice(uint256 percentDrop) external {
        require(percentDrop > 0 && percentDrop <= 100, "Invalid percentage");
        _answer = _answer * int256(100 - percentDrop) / 100;
        _roundId++;
        _updatedAt = block.timestamp;
        emit AnswerUpdated(_answer, _roundId, block.timestamp);
    }

    /// @notice Tăng giá theo phần trăm (dùng để recover)
    /// @param percentIncrease Phần trăm tăng. VD: 50 = tăng 50%
    function raisePrice(uint256 percentIncrease) external {
        _answer = _answer * int256(100 + percentIncrease) / 100;
        _roundId++;
        _updatedAt = block.timestamp;
        emit AnswerUpdated(_answer, _roundId, block.timestamp);
    }

    // ============================================================
    // Chainlink AggregatorV3Interface implementation
    // ============================================================

    function latestAnswer() external view returns (int256) {
        return _answer;
    }

    function latestRoundData() external view returns (
        uint80 roundId,
        int256 answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80 answeredInRound
    ) {
        return (_roundId, _answer, _updatedAt, _updatedAt, _roundId);
    }

    function getRoundData(uint80) external view returns (
        uint80 roundId,
        int256 answer,
        uint256 startedAt,
        uint256 updatedAt,
        uint80 answeredInRound
    ) {
        return (_roundId, _answer, _updatedAt, _updatedAt, _roundId);
    }

    function decimals() external view returns (uint8) {
        return _decimals;
    }

    function description() external view returns (string memory) {
        return _description;
    }

    function version() external view returns (uint256) {
        return _version;
    }
}
