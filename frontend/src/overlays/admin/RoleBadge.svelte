<script lang="ts">
// ユーザーロールのバッジ（旧 admin-role-badge role-{role}）。
// ラベルは大文字表記（旧 role.toUpperCase()）。
interface Props {
	role: string;
}
let { role }: Props = $props();
const normalized = $derived(role || "user");
</script>

<span class="admin-role-badge role-{normalized}">{normalized.toUpperCase()}</span>
