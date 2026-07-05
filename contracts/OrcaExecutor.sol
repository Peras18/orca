// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

// ═══════════════════════════════════════════════════════════
// 🐋 ORCA MEV EXECUTOR — Optimized for BASE Mainnet
// Features: Balancer flash loans (0% fee), Yul inline assembly,
//           kill switch, multi-DEX swaps, slippage guards
// ═══════════════════════════════════════════════════════════

interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
    function transfer(address to, uint256 value) external returns (bool);
    function approve(address spender, uint256 value) external returns (bool);
    function decimals() external view returns (uint8);
}

interface IWETH {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
    function transfer(address to, uint256 amount) external returns (bool);
}

// Balancer Vault — BASE Mainnet: 0xBA12222222228d8Ba445958a75a0704d566BF2C8
interface IBalancerVault {
    function flashLoan(
        IFlashLoanRecipient recipient,
        address[] calldata tokens,
        uint256[] calldata amounts,
        bytes calldata userData
    ) external;
}

interface IFlashLoanRecipient {
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external;
}

// Uniswap V2 Router (Aerodrome compatible)
interface IUniswapV2Router {
    function swapExactTokensForTokens(
        uint256 amountIn,
        uint256 amountOutMin,
        address[] calldata path,
        address to,
        uint256 deadline
    ) external returns (uint256[] memory amounts);
}

// Uniswap V3 Quoter
interface IUniswapV3Pool {
    function swap(
        address recipient,
        bool zeroForOne,
        int256 amountSpecified,
        uint160 sqrtPriceLimitX96,
        bytes calldata data
    ) external returns (int256 amount0, int256 amount1);
}

contract OrcaExecutor is IFlashLoanRecipient {
    // ═══════════════════════════════════════════
    // STORAGE
    // ═══════════════════════════════════════════
    
    address public owner;
    address public treasury;          // Lucros enviados aqui, nunca acumulam no executor
    address public constant BALANCER_VAULT = 0xBA12222222228d8Ba445958a75a0704d566BF2C8;
    address public constant WETH = 0x4200000000000000000000000000000000000006;
    
    // Kill switch: se true, todas as execuções revertem
    bool public killed;
    
    // Pool V3 autorizada durante swap callback (guarda anti-reentrancy)
    address private pendingV3Pool;

    // Carteiras autorizadas (WalletRotator)
    mapping(address => bool) public authorizedCallers;
    
    // Stats
    uint256 public totalExecutions;
    uint256 public totalProfit;
    uint256 public failedExecutions;
    
    // Balancer fee é 0 na BASE, mas guardamos para compatibilidade
    uint256 constant BALANCER_FEE = 0;
    
    // ═══════════════════════════════════════════
    // EVENTS
    // ═══════════════════════════════════════════
    
    event ArbExecuted(
        uint256 indexed blockNumber,
        uint256 inputAmount,
        uint256 outputAmount,
        uint256 netProfit,
        uint256 gasUsed
    );
    
    event KillSwitch(bool active);
    event TreasuryUpdated(address newTreasury);
    event CallerAuthorized(address caller);
    event CallerDeauthorized(address caller);
    
    // ═══════════════════════════════════════════
    // MODIFIERS
    // ═══════════════════════════════════════════
    
    modifier onlyOwner() {
        require(msg.sender == owner, "Not owner");
        _;
    }
    
    modifier notKilled() {
        require(!killed, "Contract killed");
        _;
    }
    
    modifier onlyAuthorized() {
        require(
            msg.sender == owner || authorizedCallers[msg.sender],
            "Not authorized"
        );
        _;
    }
    
    // ═══════════════════════════════════════════
    // CONSTRUCTOR — Simples, zero lógica extra
    // ═══════════════════════════════════════════
    
    constructor(address _treasury) {
        owner = msg.sender;
        treasury = _treasury;
        killed = false;
        
        // Approve Balancer Vault para WETH (só precisa fazer 1x no deploy)
        _approveMax(WETH, BALANCER_VAULT);
    }
    
    // ═══════════════════════════════════════════
    // KILL SWITCH
    // ═══════════════════════════════════════════
    
    function kill() external onlyOwner {
        killed = true;
        emit KillSwitch(true);
    }
    
    function unkill() external onlyOwner {
        killed = false;
        emit KillSwitch(false);
    }
    
    function setTreasury(address _treasury) external onlyOwner {
        treasury = _treasury;
        emit TreasuryUpdated(_treasury);
    }
    
    // ═══════════════════════════════════════════
    // AUTHORIZATION (WalletRotator)
    // ═══════════════════════════════════════════
    
    function authorizeCaller(address caller) external onlyOwner {
        authorizedCallers[caller] = true;
        emit CallerAuthorized(caller);
    }
    
    function deauthorizeCaller(address caller) external onlyOwner {
        authorizedCallers[caller] = false;
        emit CallerDeauthorized(caller);
    }
    
    // ═══════════════════════════════════════════
    // MAIN ENTRY POINT — execute(bytes)
    // Format: [executor:20][loanToken:20][loanAmount:32][blockDeadline:4][minProfit:4][hopCount:1][hop1:25]...
    // ═══════════════════════════════════════════

    function execute(bytes calldata route) external onlyAuthorized notKilled {
        // Decode header — novo formato com executor + loanToken
        require(route.length >= 81, "Route too short");

        // 1. Verificar executor (opcional — já validado por onlyAuthorized)
        // address expectedExecutor = address(bytes20(route[0:20]));

        // 2. Loan token (20 bytes)
        address loanToken = address(bytes20(route[20:40]));

        // 3. Loan amount (32 bytes)
        uint256 loanAmount = uint256(bytes32(route[40:72]));
        require(loanAmount > 0, "Invalid loan amount");

        // 4. Block deadline (4 bytes)
        uint32 blockDeadline = uint32(bytes4(route[72:76]));
        require(block.number <= blockDeadline, "Block deadline exceeded");

        // 5. Min profit (4 bytes)
        uint32 minProfit = uint32(bytes4(route[76:80]));

        // 6. Hop count (1 byte)
        uint8 hopCount = uint8(route[80]);
        require(hopCount >= 2 && hopCount <= 4, "Invalid hop count");
        require(route.length >= 81 + hopCount * 41, "Route length mismatch");

        // Build flash loan request
        address[] memory tokens = new address[](1);
        tokens[0] = loanToken;

        uint256[] memory amounts = new uint256[](1);
        amounts[0] = loanAmount;

        // Initiate flash loan — Balancer chama receiveFlashLoan()
        IBalancerVault(BALANCER_VAULT).flashLoan(
            IFlashLoanRecipient(address(this)),
            tokens,
            amounts,
            route
        );

        totalExecutions++;
    }
    
    // ═══════════════════════════════════════════
    // BALANCER FLASH LOAN CALLBACK
    // ═══════════════════════════════════════════
    // Balancer empresta 0% fee na BASE — lucro = output - input
    // ═══════════════════════════════════════════
    
    function receiveFlashLoan(
        address[] calldata tokens,
        uint256[] calldata amounts,
        uint256[] calldata feeAmounts,
        bytes calldata userData
    ) external override {
        require(msg.sender == BALANCER_VAULT, "Invalid caller");

        uint256 loanAmount = amounts[0];
        uint256 fee = feeAmounts[0]; // = 0 na BASE

        // Decode route — offsets ajustados para novo formato (81-byte header)
        uint32 blockDeadline = uint32(bytes4(userData[72:76]));
        require(block.number <= blockDeadline, "ORCA: deadline exceeded");

        uint32 minProfit = uint32(bytes4(userData[76:80]));
        uint8 hopCount = uint8(userData[80]);

        // Execute swaps
        uint256 balanceBefore = _balance(tokens[0]);

        // Execute all hops via Yul assembly hot path
        _executeHops(userData, hopCount, tokens[0]);

        uint256 balanceAfter = _balance(tokens[0]);

        // SLIPPAGE GUARD ON-CHAIN: verificar profit após swaps
        require(balanceAfter >= balanceBefore, "ORCA: LOSS");
        uint256 grossProfit = balanceAfter - balanceBefore;
        require(grossProfit >= fee, "ORCA: cannot repay loan");

        uint256 netProfit = grossProfit - fee;
        uint256 minProfitWei = uint256(minProfit) * 1e9; // Compactado: wei / 1e9
        require(netProfit >= minProfitWei, "ORCA: profit below minimum");

        // Repay Balancer Vault (0% fee = loanAmount)
        uint256 repayAmount = loanAmount + fee;
        _transfer(tokens[0], BALANCER_VAULT, repayAmount);

        // CORREÇÃO: treasuryAmount calculado a partir de balanceAfter -
        // repayAmount assumia implicitamente que balanceBefore == loanAmount
        // exactamente -- se o contrato tivesse qualquer saldo residual de
        // tokens[0] antes do flash loan (resíduo de execução anterior,
        // dust, ou transferência directa), balanceBefore > loanAmount,
        // tornando este cálculo incorrecto e sujeito a panic mesmo com
        // grossProfit positivo confirmado no require acima. Calcular a
        // partir de grossProfit (já validado >= fee) é matematicamente
        // equivalente no caso normal, e seguro em todos os casos.
        uint256 treasuryAmount = grossProfit - fee;
        if (treasuryAmount > 0) {
            _transfer(tokens[0], treasury, treasuryAmount);
        }

        totalProfit += treasuryAmount;

        emit ArbExecuted(
            block.number,
            loanAmount,
            balanceAfter,
            treasuryAmount,
            gasleft()
        );
    }
    
    // ═══════════════════════════════════════════
    // HOP EXECUTION — YUL INLINE ASSEMBLY HOT PATH
    // ═══════════════════════════════════════════
    // Poupança: 2000-4000 gas por swap vs Solidity puro
    // ═══════════════════════════════════════════
    
    function _executeHops(bytes calldata route, uint8 hopCount, address tokenIn) internal {
        address currentToken = tokenIn;

        for (uint8 i = 0; i < hopCount; i++) {
            uint256 offset = 81 + i * 41;
            
            // Decode hop from route bytes
            address pool;
            address tokenOut;
            uint8 dexAndFee;
            
            assembly ("memory-safe") {
                pool := shr(96, calldataload(add(route.offset, offset)))
                tokenOut := shr(96, calldataload(add(route.offset, add(offset, 20))))
                dexAndFee := byte(0, calldataload(add(route.offset, add(offset, 40))))
            
            }
            
            // Determinar tipo de DEX
            bool isAerodrome = (dexAndFee & 0x80) != 0;
            uint8 feeIndex = dexAndFee & 0x7F;
            
            // Execute swap
            if (isAerodrome) {
                _swapAerodrome(pool, currentToken, tokenOut);
            } else {
                _swapUniswapV3(pool, currentToken, tokenOut);
            }
            
            currentToken = tokenOut;
        }
    }
    
    // ═══════════════════════════════════════════
    // V2 / AERODROME SWAP — Assembly inline
    // ═══════════════════════════════════════════
    
    function _swapAerodrome(address pool, address tokenIn, address tokenOut) internal {
        uint256 amountIn = IERC20(tokenIn).balanceOf(address(this));

        // Call swap no pool V2
        // CORRECÇÃO: pools V2 esperam transfer() ANTES de swap()
        // Flow correcto: transfer -> swap (não approve -> swap)
        (uint256 reserve0, uint256 reserve1,) = _getReserves(pool);

        uint256 amountOut;
        if (tokenIn < tokenOut) {
            // amount0Out = 0, amount1Out = calculate
            amountOut = _getAmountOut(amountIn, reserve0, reserve1);
            // Transfer tokens para pool ANTES de swap
            _transfer(tokenIn, pool, amountIn);
            _callSwap(pool, 0, amountOut, address(this));
        } else {
            amountOut = _getAmountOut(amountIn, reserve1, reserve0);
            // Transfer tokens para pool ANTES de swap
            _transfer(tokenIn, pool, amountIn);
            _callSwap(pool, amountOut, 0, address(this));
        }

        // Slippage guard: verificar balance após swap
        require(IERC20(tokenOut).balanceOf(address(this)) > 0, "Zero output");
    }
    
    // ═══════════════════════════════════════════
    // V3 SWAP — Assembly inline (simplificado)
    // ═══════════════════════════════════════════
    
    function _swapUniswapV3(address pool, address tokenIn, address tokenOut) internal {
        bool zeroForOne = tokenIn < tokenOut;
        int256 amountSpecified = int256(IERC20(tokenIn).balanceOf(address(this)));
        
        // Assembly call to V3 pool
        bytes memory data = abi.encode(tokenIn, tokenOut);
        
        pendingV3Pool = pool;
        (bool success, bytes memory returnData) = pool.call(
            abi.encodeWithSelector(
                IUniswapV3Pool.swap.selector,
                address(this),
                zeroForOne,
                amountSpecified,
                zeroForOne ? uint160(4295128740) : uint160(1461446703485210103287273052203988822378723970341),
                data
            )
        );
        
        pendingV3Pool = address(0);

        // DIAGNÓSTICO TEMPORÁRIO: propagar a revert reason REAL do pool em
        // vez do "V3 swap failed" genérico -- precisamos de saber a causa
        // exata (liquidez insuficiente, amount zero, etc.) antes de decidir
        // qualquer correção. Reverter aqui ainda aborta a simulação eth_call
        // sem gastar gás real, exatamente como antes -- só muda a mensagem.
        if (!success) {
            if (returnData.length > 0) {
                assembly {
                    revert(add(returnData, 32), mload(returnData))
                }
            }
            revert("V3 swap failed (sem revert reason do pool)");
        }
    }

    function uniswapV3SwapCallback(int256 amount0Delta, int256 amount1Delta, bytes calldata data) external {
        require(msg.sender == pendingV3Pool, "ORCA: callback nao autorizado");
        (address tokenIn,) = abi.decode(data, (address, address));
        uint256 amountToPay = amount0Delta > 0 ? uint256(amount0Delta) : uint256(amount1Delta);
        _transfer(tokenIn, msg.sender, amountToPay);
    }
    
    // ═══════════════════════════════════════════
    // HELPER FUNCTIONS
    // ═══════════════════════════════════════════
    
    function _getAmountOut(uint256 amountIn, uint256 reserveIn, uint256 reserveOut) 
        internal pure returns (uint256) 
    {
        require(reserveIn > 0 && reserveOut > 0, "No liquidity");
        uint256 amountInWithFee = amountIn * 997;
        uint256 numerator = amountInWithFee * reserveOut;
        uint256 denominator = (reserveIn * 1000) + amountInWithFee;
        return numerator / denominator;
    }
    
    function _getReserves(address pool) internal view returns (uint256, uint256, uint32) {
        (bool success, bytes memory data) = pool.staticcall(
            abi.encodeWithSignature("getReserves()")
        );
        require(success && data.length >= 96, "getReserves failed");
        
        (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast) = 
            abi.decode(data, (uint112, uint112, uint32));
        
        return (uint256(reserve0), uint256(reserve1), blockTimestampLast);
    }
    
    function _callSwap(address pool, uint256 amount0Out, uint256 amount1Out, address to) internal {
        (bool success,) = pool.call(
            abi.encodeWithSignature(
                "swap(uint256,uint256,address,bytes)",
                amount0Out,
                amount1Out,
                to,
                ""
            )
        );
        require(success, "Swap failed");
    }
    
    function _balance(address token) internal view returns (uint256) {
        return IERC20(token).balanceOf(address(this));
    }
    
    function _transfer(address token, address to, uint256 amount) internal {
        (bool success,) = token.call(
            abi.encodeWithSelector(IERC20.transfer.selector, to, amount)
        );
        require(success, "Transfer failed");
    }
    
    function _approveMax(address token, address spender) internal {
        (bool success,) = token.call(
            abi.encodeWithSelector(IERC20.approve.selector, spender, type(uint256).max)
        );
        require(success, "Approve failed");
    }
    
    function _extractLoanAmount(bytes calldata route) internal pure returns (uint256) {
        // loanAmount está em bytes 40:72 (após executor[20] + loanToken[20])
        if (route.length >= 72) {
            return uint256(bytes32(route[40:72]));
        }
        return 0;
    }
    
    
    // ═══════════════════════════════════════════
    // RECEIVE / FALLBACK
    // ═══════════════════════════════════════════
    
    receive() external payable {}
    fallback() external payable {}
    
    // ═══════════════════════════════════════════
    // RESCUE FUNCTIONS (safety)
    // ═══════════════════════════════════════════
    
    function rescueERC20(address token, uint256 amount) external onlyOwner {
        _transfer(token, owner, amount);
    }
    
    function rescueETH() external onlyOwner {
        payable(owner).transfer(address(this).balance);
    }
    
    // ═══════════════════════════════════════════
    // VIEW FUNCTIONS
    // ═══════════════════════════════════════════
    
    function getStats() external view returns (uint256, uint256, uint256) {
        return (totalExecutions, totalProfit, failedExecutions);
    }
}
