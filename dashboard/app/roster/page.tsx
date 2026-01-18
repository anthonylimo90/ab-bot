import { redirect } from 'next/navigation';

export default function RosterPage() {
  redirect('/trading?tab=active');
}
