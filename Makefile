.PHONY: build idl test
build:
	cargo-build-sbf --tools-version v1.52

idl:
	RUSTUP_TOOLCHAIN=nightly anchor idl build -p sendguard -o idl/sendguard.json
	node -e "const i=require('./idl/sendguard.json');require('fs').writeFileSync('web/idl.js','window.SENDGUARD_IDL = '+JSON.stringify(i)+';\\n')"

test: build
	cd tests-host && cargo test
