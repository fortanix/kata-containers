#!/usr/bin/env bats
# Copyright (c) Fortanix, Inc.
#
# SPDX-License-Identifier: Apache-2.0
#

load "${BATS_TEST_DIRNAME}/lib.sh"
load "${BATS_TEST_DIRNAME}/confidential_common.sh"
load "${BATS_TEST_DIRNAME}/confidential_ccm.sh"

export CCM="${CCM:-false}"
export CCM_UNENCRYPTED_IMAGE="${CCM_UNENCRYPTED_IMAGE:-docker.io/library/busybox:1.36}"

setup() {
	is_confidential_runtime_class || skip "Test not supported for ${KATA_HYPERVISOR}."

	if [ "${CCM}" = "false" ]; then
		skip "Test skipped as CCM not setup"
	fi

	is_confidential_hardware || skip "Test requires SNP or TDX hardware"

	[ "${SNAPSHOTTER:-}" = "nydus" ] || skip "None snapshotter was found but this test requires one"

	setup_common || die "setup_common failed"
	ccm_check_env || die "CCM / DSM settings are missing"
	ccm_check_gpu || die "GPU runtime class selected but no GPU is available"

	policy_settings_dir="$(create_tmp_policy_settings_dir "${pod_config_dir}")"
}

@test "Create a pod from a DSM-encrypted image with valid initdata" {
	initdata="$(get_initdata_ccm)"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	k8s_create_pod "${kata_pod}"
}

@test "Pod with incorrect initdata should not start" {
	initdata="$(get_initdata_ccm "# incorrect")"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	assert_pod_fail "${kata_pod}"
	assert_logs_contain "${node}" kata "${node_start_time}" 'Failed to decrypt the image layer'
}

@test "Pod with a changed guest measurement should not start" {
	initdata="$(get_initdata_ccm)"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params agent.log=debug)" "${initdata}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	assert_pod_fail "${kata_pod}"
	assert_logs_contain "${node}" kata "${node_start_time}" 'Failed to decrypt the image layer'
}

@test "Pod from a DSM-encrypted image without initdata should not start" {
	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" ""
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	assert_pod_fail "${kata_pod}"
	assert_logs_contain "${node}" kata "${node_start_time}" 'Failed to decrypt the image layer'
}

@test "Create a pod from an unencrypted image with CCM initdata" {
	initdata="$(get_initdata_ccm)"

	ccm_create_pod_yaml "${CCM_UNENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	k8s_create_pod "${kata_pod}"
}

@test "Pod with an invalid runtimeClassName is rejected" {
	initdata="$(get_initdata_ccm)"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	yq -i '.spec.runtimeClassName = "kata-invalid"' "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	run kubectl apply -f "${kata_pod}"
	echo "${output}"

	[ "${status}" -ne 0 ]
	[[ "${output}" == *'RuntimeClass "kata-invalid" not found'* ]]
}

@test "Create a pod from a DSM-encrypted image with a workflow AppConfig ID" {
	[ -n "${CCM_APPCONFIG_ID}" ] || skip "CCM_APPCONFIG_ID is not set"

	initdata="$(get_initdata_ccm_unmeasured_enabled)"
	unmeasured_initdata="$(get_unmeasured_initdata_ccm)"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	yq -i ".metadata.annotations.\"io.katacontainers.config.hypervisor.cc_unmeasured_init_data\" = \"${unmeasured_initdata}\"" "${kata_pod}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	k8s_create_pod "${kata_pod}"
}

@test "Pod with an unknown workflow AppConfig ID should not start" {
	[ -n "${CCM_APPCONFIG_ID}" ] || skip "CCM_APPCONFIG_ID is not set"

	local unknown_appconfig_id
	unknown_appconfig_id="$(printf '0%.0s' {1..64})"

	initdata="$(get_initdata_ccm_unmeasured_enabled)"
	unmeasured_initdata="$(get_unmeasured_initdata_ccm "${unknown_appconfig_id}")"

	ccm_create_pod_yaml "${DSM_ENCRYPTED_IMAGE}" "$(ccm_kernel_params)" "${initdata}"
	yq -i ".metadata.annotations.\"io.katacontainers.config.hypervisor.cc_unmeasured_init_data\" = \"${unmeasured_initdata}\"" "${kata_pod}"
	auto_generate_policy "${policy_settings_dir}" "${kata_pod}"

	echo "Pod ${kata_pod}: $(cat ${kata_pod})"

	assert_pod_fail "${kata_pod}"
	assert_logs_contain "${node}" kata "${node_start_time}" 'Failed to decrypt the image layer'
}

teardown() {
	is_confidential_runtime_class || skip "Test not supported for ${KATA_HYPERVISOR}."

	if [ "${CCM}" = "false" ]; then
		skip "Test skipped as CCM not setup"
	fi

	is_confidential_hardware || skip "Test requires SNP or TDX hardware"

	[ "${SNAPSHOTTER:-}" = "nydus" ] || skip "None snapshotter was found but this test requires one"

	delete_tmp_policy_settings_dir "${policy_settings_dir:-}"
	teardown_common "${node}" "${node_start_time:-}"
}
