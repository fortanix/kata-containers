#!/usr/bin/env bash
# Copyright (c) Fortanix, Inc.
#
# SPDX-License-Identifier: Apache-2.0
#
set -e

DSM_ENDPOINT="${DSM_ENDPOINT:-}"
DSM_APP_ID="${DSM_APP_ID:-}"
CCM_DOMAIN_NAMES="${CCM_DOMAIN_NAMES:-}"
CCM_APPCONFIG_ID="${CCM_APPCONFIG_ID:-}"
DSM_ENCRYPTED_IMAGE="${DSM_ENCRYPTED_IMAGE:-}"

ccm_check_env() {
	local var
	for var in DSM_ENDPOINT DSM_APP_ID CCM_DOMAIN_NAMES DSM_ENCRYPTED_IMAGE; do
		if [[ -z "${!var}" ]]; then
			echo "${var} is not set" >&2
			return 1
		fi
	done
}

ccm_kernel_params() {
	local params="${1:-}"

	echo "agent.guest_components_rest_api=all${params:+ ${params}}"
}

ccm_check_gpu() {
	local available

	is_confidential_gpu_hypervisor || return 0
	available="$(kubectl get node "${node}" -o jsonpath='{.status.allocatable.nvidia\.com/pgpu}')"
	if [[ ! "${available}" =~ ^[1-9][0-9]*$ ]]; then
		echo "${node} has no allocatable nvidia.com/pgpu" >&2
		return 1
	fi
}

ccm_create_pod_yaml() {
	local image="$1"
	local kernel_params="$2"
	local initdata="$3"

	create_coco_pod_yaml_with_annotations "${image}" "${kernel_params}" "${initdata}" "${node}"

	is_confidential_gpu_hypervisor || return 0
	yq -i '
		.spec.containers[0].resources.limits."nvidia.com/pgpu" = "1" |
		.spec.containers[0].resources.limits.memory = "16Gi" |
		.spec.containers[0].securityContext.privileged = true
	' "${kata_pod}"
}

_ccm_domain_names_toml() {
	local IFS=","
	local names=()
	local n
	for n in ${CCM_DOMAIN_NAMES}; do
		names+=("\"${n}\"")
	done
	echo "[${names[*]}]"
}

_ccm_initdata_toml() {
	local cdh_extra="${1:-}"

	cat << EOF
version = "0.1.0"
algorithm = "sha256"
[data]
"aa.toml" = '''
[token_configs.ccm_as]
ccm_domain_names = $(_ccm_domain_names_toml)
'''

"cdh.toml" = '''
[kbc]
name = "ccm_kbc"
url = "${DSM_ENDPOINT}"

[kbc_configs.ccm_kbc]
dsm_app_id = "${DSM_APP_ID}"
${cdh_extra}
'''
EOF
}

get_initdata_ccm() {
	_ccm_initdata_toml "${1:-}" | gzip -c | base64 -w0
}

get_initdata_ccm_unmeasured_enabled() {
	{
		_ccm_initdata_toml
		cat << EOF

"unmeasured-initdata-cfg.toml" = '''
enabled = true
'''
EOF
	} | gzip -c | base64 -w0
}

get_unmeasured_initdata_ccm() {
	local appconfig_id="${1:-${CCM_APPCONFIG_ID}}"

	gzip -c << EOF | base64 -w0
version = "0.1.0"

[data]
"aa.toml" = '''
[ccm_as]
ccm_appconfig_id = "${appconfig_id}"
'''
EOF
}
