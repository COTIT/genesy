import { ProductBulkDetail } from "@/components/product-bulk/detail"

export default async function Page({
  params,
}: {
  params: Promise<{ id: string }>
}) {
  const { id } = await params
  return <ProductBulkDetail id={id} />
}
