import { request } from "./http";

export interface OrderDto {
  qty: number;
  price: number;
}

export function fetchOrders(): Promise<OrderDto[]> {
  return request<OrderDto[]>("/api/orders");
}
