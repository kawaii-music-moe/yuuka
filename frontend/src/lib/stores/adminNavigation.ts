import { writable } from "svelte/store";

export const adminNavigationOpen = writable(false);

export function openAdminNavigation(): void {
	adminNavigationOpen.set(true);
}

export function closeAdminNavigation(): void {
	adminNavigationOpen.set(false);
}
