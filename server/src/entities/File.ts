import { Entity, Column, PrimaryColumn, ManyToOne, JoinColumn } from "typeorm";
import { User } from "./User";
import { Group } from "./Group";

@Entity()
export class File {
  @PrimaryColumn()
  path: string;

  @ManyToOne(() => User, (user) => user.files)
  @JoinColumn({ name: "owner", referencedColumnName: "uid" })
  owner: User;

  @Column()
  type: number; // 0 = file, 1 = directory, 2 = symlink, etc.

  @Column()
  permissions: number;

  @ManyToOne(() => Group, (group) => group.gid, { nullable: true })
  @JoinColumn({ name: "group" })
  group: Group;

  @Column()
  atime: number; // last access time

  @Column()
  mtime: number; // last modification time

  @Column()
  ctime: number; // last time that file's metadata (e.g., permissions) was last changed

  @Column()
  btime: number; // birth time
}