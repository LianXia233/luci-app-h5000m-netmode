# 这个 Makefile 不编译任何东西，它只是 feeds/luci/luci.mk 的一个“判定补丁”。
#
# luci.mk 中两个分支的判断条件并不一致：
#   Build/Compile        依据 $(wildcard ${CURDIR}/src/Makefile) —— 要求“有 Makefile”
#   Package/.../install 依据 $(wildcard ${CURDIR}/src)          —— 只要“目录存在”
# 后端重写为 Rust crate 之后，src/ 下只有 .rs 源码、没有 Makefile，于是踩中这个组合：
#   - Compile 分支被跳过，ipkg-install 目录永远不会被创建；
#   - install 分支却照样执行 Build/Install/Default，即
#     make -C $(PKG_BUILD_DIR)/src install，而该目录顶层没有 Makefile，
#     make 报 "*** No rule to make target 'install'.  Stop." 并以 exit code 2 退出，
#     表现为 luci.mk 末尾 BuildPackage 展开的打包规则报错，整个 world 编译随之中断。
#     2026-09-29 下游各云编译仓库（H5000M / AP3000M / x86_64 全矩阵）同时失败即此因。
#
# OpenWrt buildroot 内没有 Rust 工具链，h5000m-netmode 与 h5000m-netmode-status
# 两个静态 ELF 由 scripts/build-rust.sh 交叉编译后随仓库发布在 root/usr/sbin/ 下
# （另见 Makefile 顶部注释）。因此这里 compile / clean 均为空操作，install 只负责把
# 已发布的 ELF 装进 DESTDIR，供 luci.mk 收集进 ipkg-install。
#
# 若将来 buildroot 内真的具备了 Rust 工具链、需要在此处 cargo build，请把 compile
# 换成真实构建命令，install 保持“把产物装进 DESTDIR”的语义即可，无需改动其他文件。

DESTDIR ?=

.PHONY: all compile clean install

all:
	@true

compile: all

clean:
	@true

install:
	mkdir -p "$(DESTDIR)/usr/sbin"
	cp -f ../root/usr/sbin/h5000m-netmode "$(DESTDIR)/usr/sbin/h5000m-netmode"
	cp -f ../root/usr/sbin/h5000m-netmode-status "$(DESTDIR)/usr/sbin/h5000m-netmode-status"
