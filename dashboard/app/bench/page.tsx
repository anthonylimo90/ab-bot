import { redirect } from 'next/navigation';

export default function BenchPage() {
  redirect('/trading?tab=watching');
}
