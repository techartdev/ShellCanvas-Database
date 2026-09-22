// SPDX-License-Identifier: MPL-2.0
/** Advance by rows actually returned because the adapter may stop a page at its byte cap. */
/** @param {{offset:number, rows:readonly unknown[]}} page */
export function nextOffset(page) { return page.offset + page.rows.length; }
