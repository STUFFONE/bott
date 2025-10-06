#!/bin/bash

# SolSniper 启动脚本

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 打印带颜色的消息
print_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 检查 .env 文件
check_env() {
    if [ ! -f ".env" ]; then
        print_error ".env file not found!"
        print_info "Creating .env from .env.example..."
        cp .env.example .env
        print_warning "Please edit .env file with your configuration before running."
        exit 1
    fi
    print_success ".env file found"
}

# 检查 Rust 安装
check_rust() {
    if ! command -v cargo &> /dev/null; then
        print_error "Rust is not installed!"
        print_info "Install Rust from: https://rustup.rs/"
        exit 1
    fi
    print_success "Rust is installed: $(rustc --version)"
}

# 编译项目
build_project() {
    local mode=$1
    
    if [ "$mode" == "release" ]; then
        print_info "Building in release mode (optimized)..."
        cargo build --release
        print_success "Build completed: ./target/release/solsniper"
    else
        print_info "Building in debug mode..."
        cargo build
        print_success "Build completed: ./target/debug/solsniper"
    fi
}

# 运行项目
run_project() {
    local mode=$1
    local log_level=${2:-info}
    
    export RUST_LOG=$log_level
    
    if [ "$mode" == "release" ]; then
        print_info "Running in release mode with log level: $log_level"
        ./target/release/solsniper
    else
        print_info "Running in debug mode with log level: $log_level"
        cargo run
    fi
}

# 显示帮助信息
show_help() {
    echo "SolSniper - Pump.fun High-Performance Sniper Bot"
    echo ""
    echo "Usage: ./run.sh [COMMAND] [OPTIONS]"
    echo ""
    echo "Commands:"
    echo "  build           Build the project (debug mode)"
    echo "  build-release   Build the project (release mode, optimized)"
    echo "  run             Build and run (debug mode)"
    echo "  run-release     Build and run (release mode)"
    echo "  start           Run without building (release mode)"
    echo "  check           Check environment and dependencies"
    echo "  clean           Clean build artifacts"
    echo "  help            Show this help message"
    echo ""
    echo "Options:"
    echo "  --log-level LEVEL   Set log level (trace, debug, info, warn, error)"
    echo "                      Default: info"
    echo ""
    echo "Examples:"
    echo "  ./run.sh build-release"
    echo "  ./run.sh run --log-level debug"
    echo "  ./run.sh run-release --log-level info"
    echo ""
}

# 清理构建产物
clean_project() {
    print_info "Cleaning build artifacts..."
    cargo clean
    print_success "Clean completed"
}

# 检查环境
check_environment() {
    print_info "Checking environment..."
    check_rust
    check_env
    print_success "Environment check passed"
}

# 主函数
main() {
    local command=${1:-help}
    local log_level="info"
    
    # 解析参数
    shift || true
    while [[ $# -gt 0 ]]; do
        case $1 in
            --log-level)
                log_level="$2"
                shift 2
                ;;
            *)
                print_error "Unknown option: $1"
                show_help
                exit 1
                ;;
        esac
    done
    
    # 执行命令
    case $command in
        build)
            check_environment
            build_project "debug"
            ;;
        build-release)
            check_environment
            build_project "release"
            ;;
        run)
            check_environment
            build_project "debug"
            run_project "debug" "$log_level"
            ;;
        run-release)
            check_environment
            build_project "release"
            run_project "release" "$log_level"
            ;;
        start)
            check_environment
            if [ ! -f "./target/release/solsniper" ]; then
                print_error "Release binary not found. Run 'build-release' first."
                exit 1
            fi
            run_project "release" "$log_level"
            ;;
        check)
            check_environment
            ;;
        clean)
            clean_project
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            print_error "Unknown command: $command"
            show_help
            exit 1
            ;;
    esac
}

# 运行主函数
main "$@"

